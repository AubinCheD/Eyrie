//go:build android

// Le nœud Tailscale de l'application Android d'Eyrie : le téléphone devient lui-même un appareil du tailnet
// (tsnet, pile réseau en espace utilisateur : ni VPN Android, ni application Tailscale), et ses connexions vers le
// PC passent par lui. Compilé en bibliothèque partagée (`-buildmode=c-shared`) par tools\android.ps1, appelé par
// crates/eyrie-mobile/src/tailnet.rs. Les chaînes rendues sont à libérer par EyrieTailnetFree.
package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"sync"
	"time"
	"unsafe"

	"eyrie-tailnet/noeud"

	"tailscale.com/client/local"
	"tailscale.com/logtail"
	"tailscale.com/net/netmon"
	"tailscale.com/tsnet"
)

// Le journal du nœud, dans son dossier, s'arrête à cette taille.
const journalMax = 4 << 20

var (
	// starting : une seule mise en route à la fois ; mu : l'état, lu sans attendre la mise en route.
	starting sync.Mutex
	mu       sync.Mutex
	srv      *tsnet.Server
	lc       *local.Client
	state    = map[string]string{"state": "stopped"}
)

func main() {}

func cerr(err error) *C.char {
	if err == nil {
		return nil
	}
	return C.CString(err.Error())
}

func report(m map[string]string) {
	mu.Lock()
	state = m
	mu.Unlock()
}

// EyrieTailnetStart démarre le nœud (une fois) : `dir` garde son état (sa clé comprise), dans le dossier privé de
// l'application ; `hostname` est son nom dans le tailnet. Rend NULL, ou l'erreur.
//
//export EyrieTailnetStart
func EyrieTailnetStart(cdir, chost *C.char) *C.char {
	dir, host := C.GoString(cdir), C.GoString(chost)
	starting.Lock()
	defer starting.Unlock()
	mu.Lock()
	started := srv != nil
	mu.Unlock()
	if started {
		return nil
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return cerr(err)
	}
	report(map[string]string{"state": "starting"})
	// Les journaux restent sur le téléphone : rien n'est envoyé à Tailscale (voir le compagnon du PC).
	os.Setenv("TS_NO_LOGS_NO_SUPPORT", "true")
	logtail.Disable()
	netmon.RegisterInterfaceGetter(interfaces)
	logf := journal(filepath.Join(dir, "journal.txt"))
	s := &tsnet.Server{Dir: dir, Hostname: host, Logf: logf, UserLogf: logf}
	if err := s.Start(); err != nil {
		s.Close()
		return cerr(err)
	}
	c, err := s.LocalClient()
	if err != nil {
		s.Close()
		return cerr(err)
	}
	mu.Lock()
	srv, lc = s, c
	mu.Unlock()
	go noeud.Suivre(context.Background(), c, report)
	return nil
}

// EyrieTailnetState rend l'état du nœud en JSON : {"state": stopped, starting, needs_login (+ url), running
// (+ name, ip) ou error (+ message)}.
//
//export EyrieTailnetState
func EyrieTailnetState() *C.char {
	mu.Lock()
	b, _ := json.Marshal(state)
	mu.Unlock()
	return C.CString(string(b))
}

// EyrieTailnetDial relie `fd` (un bout de paire de sockets de l'application, dont la bibliothèque devient
// propriétaire) à `addr` sur le tailnet, en `timeoutMs` au plus. Rend NULL une fois relié, ou l'erreur (`fd` est
// alors fermé).
//
//export EyrieTailnetDial
func EyrieTailnetDial(caddr *C.char, fd C.int, timeoutMs C.int) *C.char {
	addr := C.GoString(caddr)
	f := os.NewFile(uintptr(fd), "eyrie")
	phone, err := net.FileConn(f)
	f.Close()
	if err != nil {
		return cerr(err)
	}
	mu.Lock()
	s := srv
	mu.Unlock()
	if s == nil {
		phone.Close()
		return cerr(errors.New("nœud Tailscale arrêté"))
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Duration(timeoutMs)*time.Millisecond)
	defer cancel()
	pc, err := s.Dial(ctx, "tcp", addr)
	if err != nil {
		phone.Close()
		return cerr(err)
	}
	go noeud.Relier(phone, pc)
	return nil
}

// EyrieTailnetLogout déconnecte le téléphone du compte Tailscale ; un nouveau lien de connexion suit.
//
//export EyrieTailnetLogout
func EyrieTailnetLogout() *C.char {
	mu.Lock()
	c := lc
	mu.Unlock()
	if c == nil {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return cerr(c.Logout(ctx))
}

//export EyrieTailnetFree
func EyrieTailnetFree(p *C.char) {
	C.free(unsafe.Pointer(p))
}

// journal écrit les lignes de tsnet dans `path`, jusqu'à journalMax.
func journal(path string) func(string, ...any) {
	f, err := os.Create(path)
	if err != nil {
		return func(string, ...any) {}
	}
	var jm sync.Mutex
	written := 0
	return func(format string, args ...any) {
		jm.Lock()
		defer jm.Unlock()
		if written < journalMax {
			n, _ := fmt.Fprintf(f, time.Now().Format("15:04:05 ")+format+"\n", args...)
			written += n
		}
	}
}

// interfaces : Android refuse aux applications la liste des interfaces réseau (netlink). Comme la fonctionnalité
// « androidbin » de Tailscale, une interface unique porte les adresses de sortie que le système choisit pour une
// connexion UDP (qui n'envoie rien). Sans réseau, la liste est vide : le nœud attend, et le sondage de Tailscale
// voit le réseau revenir.
func interfaces() ([]netmon.Interface, error) {
	var addrs []net.Addr
	if ip := outbound("udp4", "8.8.8.8:53"); ip != nil {
		addrs = append(addrs, &net.IPNet{IP: ip, Mask: net.CIDRMask(32, 32)})
	}
	if ip := outbound("udp6", "[2001:4860:4860::8888]:53"); ip != nil {
		addrs = append(addrs, &net.IPNet{IP: ip, Mask: net.CIDRMask(128, 128)})
	}
	if len(addrs) == 0 {
		return nil, nil
	}
	return []netmon.Interface{{
		Interface: &net.Interface{Index: 1, MTU: 1500, Name: "android", Flags: net.FlagUp | net.FlagRunning},
		AltAddrs:  addrs,
	}}, nil
}

func outbound(network, addr string) net.IP {
	c, err := net.DialTimeout(network, addr, 2*time.Second)
	if err != nil {
		return nil
	}
	defer c.Close()
	ua, ok := c.LocalAddr().(*net.UDPAddr)
	if !ok || ua.IP.IsLoopback() || ua.IP.IsUnspecified() {
		return nil
	}
	if network == "udp4" {
		return ua.IP.To4()
	}
	return ua.IP
}
