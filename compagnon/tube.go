//go:build windows

package main

// Le veilleur (`--veille`) garde le nœud d'Eyrie sur le tailnet quand elle est fermée. Eyrie ne le lance pas : elle
// s'y rattache, par un tube nommé réservé au compte de l'utilisateur. Les tubes sont communs à tout le poste : un nom
// qu'un autre compte devine (tiré du dossier du nœud), il le prendrait avant le veilleur, qui ne démarrerait plus.
// Le nom est donc tiré au hasard à chaque démarrage et écrit dans le dossier du nœud (pipeFile), qu'Eyrie lit et que
// les autres comptes ne lisent pas (%LOCALAPPDATA%). Le tube parle comme
// l'entrée et la sortie du compagnon lancé par Eyrie : consignes de relais et {"arret":true} dans un sens, lignes
// d'état dans l'autre. Son client est vérifié (même compte Windows ; l'exécutable d'Eyrie ou du compagnon, rangé à
// côté du veilleur) ; un seul à la fois. Eyrie ouvre le tube au niveau SECURITY_IDENTIFICATION, sans quoi, lancée en
// administrateur, elle ne serait pas reconnue. Le client parti (Eyrie fermée, arrêtée ou plantée), plus aucune
// connexion n'est relayée (celles en cours vont jusqu'au bout, qu'Eyrie ferme), et le nœud reste sur le tailnet.
//
// Le veilleur prend le verrou du nœud (`verrou.go`) avant d'ouvrir son tube et d'en écrire le nom : un instant, le
// verrou est pris sans tube (ou avec le nom du tube d'un veilleur précédent, fermé). Eyrie qui trouve le verrou pris
// sans pouvoir ouvrir le tube réessaie donc quelques secondes, en relisant le nom, avant de conclure qu'un autre
// compagnon tient le nœud.

import (
	"bufio"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"time"

	"github.com/tailscale/go-winio"
	"golang.org/x/sys/windows"
)

// Le début du nom du tube d'un veilleur ; la suite est tirée au hasard (newPipeName).
const pipePrefix = `\\.\pipe\eyrie-tailnet-`

// pipeFile : dans le dossier du nœud, le nom du tube du veilleur qui le tient, sur une ligne (PIPE_FILE de
// crates/eyrie/src/veille.rs).
const pipeFile = "tube.txt"

// newPipeName : un nom de tube neuf, pipePrefix suivi de 16 octets au hasard en hexadécimal.
var newPipeName = func() (string, error) {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return pipePrefix + hex.EncodeToString(b), nil
}

// controlPipe : le tube du veilleur du nœud `dir`, lu dans pipeFile ; "" sans fichier, ou si ce n'est pas un nom que
// tire newPipeName (ni un tube d'un autre poste, ni autre chose).
func controlPipe(dir string) string {
	raw, err := os.ReadFile(filepath.Join(dir, pipeFile))
	if err != nil {
		return ""
	}
	name := strings.TrimSpace(string(raw))
	if suffix, ok := strings.CutPrefix(name, pipePrefix); !ok || !lowerHex(suffix, 32) {
		return ""
	}
	return name
}

// writePipeFile écrit le nom `name` dans pipeFile, d'un coup (fichier temporaire renommé par-dessus) : Eyrie ne lit
// jamais un nom à moitié écrit.
func writePipeFile(dir, name string) error {
	tmp := filepath.Join(dir, pipeFile+".tmp")
	if err := os.WriteFile(tmp, []byte(name+"\n"), 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, filepath.Join(dir, pipeFile))
}

// userSID : le compte Windows du processus.
func userSID() (*windows.SID, error) {
	u, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return nil, fmt.Errorf("compte de l'utilisateur illisible : %w", err)
	}
	return u.User.Sid, nil
}

// listenControl ouvre le tube du nœud `dir` sous un nom neuf, réservé au compte de l'utilisateur (liste de contrôle
// protégée, sans héritage) et fermé aux clients d'un autre poste, puis en écrit le nom dans pipeFile. Un tube déjà
// ouvert sous ce nom (personne ne peut le créer avant le veilleur pour se faire passer pour lui) : un autre nom, trois
// essais au plus.
func listenControl(dir string) (net.Listener, error) {
	me, err := userSID()
	if err != nil {
		return nil, err
	}
	cfg := &winio.PipeConfig{
		SecurityDescriptor: "D:P(A;;GA;;;" + me.String() + ")",
		// Sans tampon, chaque ligne d'état attendrait qu'Eyrie la lise.
		InputBufferSize:  pipeBuffer,
		OutputBufferSize: pipeBuffer,
	}
	var last error
	for range 3 {
		name, err := newPipeName()
		if err != nil {
			return nil, err
		}
		ln, err := winio.ListenPipe(name, cfg)
		if err != nil {
			last = err
			continue
		}
		if err := writePipeFile(dir, name); err != nil {
			ln.Close()
			return nil, fmt.Errorf("%s : %w", pipeFile, err)
		}
		return ln, nil
	}
	return nil, last
}

// pipeHandle : le handle d'une connexion par tube.
func pipeHandle(c net.Conn) (windows.Handle, error) {
	f, ok := c.(interface{ Fd() uintptr })
	if !ok {
		return 0, errors.New("connexion sans handle")
	}
	return windows.Handle(f.Fd()), nil
}

// dialControl ouvre le tube `name` en se laissant identifier (SECURITY_IDENTIFICATION) : le veilleur lit alors le
// compte du client sur le tube, même pour un processus lancé en administrateur (checkClient).
func dialControl(name string, timeout time.Duration) (net.Conn, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return winio.DialPipeAccessImpLevel(ctx, name, windows.GENERIC_READ|windows.GENERIC_WRITE,
		winio.PipeImpLevelIdentification)
}

// checkClient vérifie le processus au bout du tube `c` (voir checkProcess). Son compte est lu sur le jeton que le
// client présente au tube (pipeClientUser) ; s'il n'en présente pas (client anonyme, ou ouvert sans qualité de
// service et encore muet), sur celui de son processus, qui n'est lisible que s'il n'est pas élevé.
func checkClient(c net.Conn, allowed []string) error {
	h, err := pipeHandle(c)
	if err != nil {
		return err
	}
	var pid uint32
	if err := windows.GetNamedPipeClientProcessId(h, &pid); err != nil {
		return fmt.Errorf("processus du client illisible : %w", err)
	}
	user, _ := pipeClientUser(h)
	return checkProcess(pid, user, allowed)
}

var impersonateNamedPipeClient = windows.NewLazySystemDLL("advapi32.dll").NewProc("ImpersonateNamedPipeClient")

// pipeClientUser : le compte du client du tube `h`, lu en empruntant son identité (au niveau identification, qui
// suffit à lire son jeton). Le jeton du processus d'une Eyrie lancée en administrateur est fermé au veilleur ; celui
// qu'elle présente au tube ne l'est pas.
func pipeClientUser(h windows.Handle) (*windows.SID, error) {
	var sid *windows.SID
	var err error
	done := make(chan struct{})
	go func() {
		defer close(done)
		// L'identité empruntée est celle du fil : la goroutine le garde jusqu'à l'avoir rendue. Si elle ne pouvait
		// pas la rendre, le fil, jamais déverrouillé, finirait avec elle.
		runtime.LockOSThread()
		if r, _, e := impersonateNamedPipeClient.Call(uintptr(h)); r == 0 {
			err = e
			runtime.UnlockOSThread()
			return
		}
		sid, err = threadUser()
		if windows.RevertToSelf() == nil {
			runtime.UnlockOSThread()
		}
	}()
	<-done
	return sid, err
}

// threadUser : le compte du jeton du fil courant (celui du client dont il emprunte l'identité).
func threadUser() (*windows.SID, error) {
	var tok windows.Token
	// Ouvert avec les droits du veilleur : au niveau identification, le fil ne peut rien ouvrir en tant que client.
	if err := windows.OpenThreadToken(windows.CurrentThread(), windows.TOKEN_QUERY, true, &tok); err != nil {
		return nil, err
	}
	defer tok.Close()
	u, err := tok.GetTokenUser()
	if err != nil {
		return nil, err
	}
	return u.User.Sid, nil
}

// checkProcess : le processus `pid` tourne-t-il sous le compte de l'utilisateur, depuis l'un des exécutables
// `allowed` ? Son compte est `user` (lu sur le tube), ou, sans lui, celui du jeton du processus. Les fichiers sont
// comparés, pas les chemins (casse, noms courts).
func checkProcess(pid uint32, user *windows.SID, allowed []string) error {
	h, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return fmt.Errorf("processus %d : %w", pid, err)
	}
	defer windows.CloseHandle(h)
	if user == nil {
		var tok windows.Token
		if err := windows.OpenProcessToken(h, windows.TOKEN_QUERY, &tok); err != nil {
			return fmt.Errorf("compte du processus %d illisible : %w", pid, err)
		}
		defer tok.Close()
		u, err := tok.GetTokenUser()
		if err != nil {
			return fmt.Errorf("compte du processus %d illisible : %w", pid, err)
		}
		user = u.User.Sid
	}
	me, err := userSID()
	if err != nil {
		return err
	}
	if !user.Equals(me) {
		return fmt.Errorf("processus %d d'un autre compte (%s)", pid, user)
	}
	image, err := processImage(h)
	if err != nil {
		return fmt.Errorf("exécutable du processus %d illisible : %w", pid, err)
	}
	for _, a := range allowed {
		if sameFile(image, a) {
			return nil
		}
	}
	return fmt.Errorf("exécutable non reconnu : %s", image)
}

// processImage : le chemin de l'exécutable du processus `h`.
func processImage(h windows.Handle) (string, error) {
	buf := make([]uint16, windows.MAX_LONG_PATH)
	n := uint32(len(buf))
	if err := windows.QueryFullProcessImageName(h, 0, &buf[0], &n); err != nil {
		return "", err
	}
	return windows.UTF16ToString(buf[:n]), nil
}

func sameFile(a, b string) bool {
	fa, err := os.Stat(a)
	if err != nil {
		return false
	}
	fb, err := os.Stat(b)
	return err == nil && os.SameFile(fa, fb)
}

// Une consigne fait au plus cette taille ; les tampons du tube.
const (
	maxControlLine = 4096
	pipeBuffer     = 64 << 10
)

// Ce que le veilleur répond à {"arret":true}, juste avant de s'arrêter (Eyrie ignore cet état).
var stoppedLine = []byte(`{"state":"stopped"}` + "\n")

// tube : le contrôle du veilleur.
type tube struct {
	relay *relay
	// check vérifie le client d'une connexion (checkClient).
	check func(net.Conn) error
	// stop arrête le veilleur.
	stop func()
	logf func(string, ...any)
	// Un client est rattaché.
	busy atomic.Bool
}

func (t *tube) serve(ctx context.Context, ln net.Listener) {
	go func() {
		<-ctx.Done()
		ln.Close()
	}()
	for {
		c, err := ln.Accept()
		if err != nil {
			if ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				return
			}
			t.logf("tube : %v", err)
			time.Sleep(200 * time.Millisecond)
			continue
		}
		if err := t.check(c); err != nil {
			t.logf("tube : client refusé : %v", err)
			c.Close()
			continue
		}
		go t.session(c)
	}
}

func lines(c net.Conn) *bufio.Scanner {
	sc := bufio.NewScanner(c)
	sc.Buffer(make([]byte, 0, 512), maxControlLine)
	return sc
}

func (t *tube) session(c net.Conn) {
	defer c.Close()
	if !t.busy.CompareAndSwap(false, true) {
		// Un seul client à la fois. Un autre n'est écouté que pour un arrêt (tools\compagnon.ps1, avant de
		// remplacer l'exécutable du veilleur).
		c.SetDeadline(time.Now().Add(5 * time.Second))
		sc := lines(c)
		if sc.Scan() {
			if stop, _ := control(&relay{}, strings.TrimSpace(sc.Text())); stop {
				t.halt(c)
				return
			}
		}
		line, _ := json.Marshal(map[string]string{"state": "error", "message": "le veilleur sert déjà une autre Eyrie"})
		_, _ = c.Write(append(line, '\n'))
		return
	}
	defer t.busy.Store(false)
	w := &timedWriter{c: c}
	status.attach(w)
	defer func() {
		status.detach(w)
		// Eyrie partie : plus rien n'est relayé ; le nœud reste sur le tailnet.
		t.relay.current.Store(nil)
	}()
	sc := lines(c)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		stop, err := control(t.relay, line)
		if err != nil {
			t.logf("consigne de relais refusée : %v", err)
		}
		if stop {
			status.detach(w)
			t.halt(c)
			return
		}
	}
}

// halt répond à un arrêt, attend (2 s au plus) que le client l'ait lu, puis arrête le veilleur.
func (t *tube) halt(c net.Conn) {
	c.SetWriteDeadline(time.Now().Add(2 * time.Second))
	if _, err := c.Write(stoppedLine); err == nil {
		if f, ok := c.(interface{ Flush() error }); ok {
			done := make(chan struct{})
			go func() {
				_ = f.Flush()
				close(done)
			}()
			select {
			case <-done:
			case <-time.After(2 * time.Second):
			}
		}
	}
	t.logf("tube : arrêt demandé")
	t.stop()
}

// timedWriter : une Eyrie qui ne lit plus ses lignes d'état ne bloque pas le veilleur ; sa séance se termine.
type timedWriter struct {
	c net.Conn
}

func (w *timedWriter) Write(b []byte) (int, error) {
	w.c.SetWriteDeadline(time.Now().Add(2 * time.Second))
	n, err := w.c.Write(b)
	if err != nil {
		w.c.Close()
	}
	return n, err
}
