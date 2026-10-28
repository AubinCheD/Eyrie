//go:build windows

package main

// Le veilleur (`--veille`) garde le nœud d'Eyrie sur le tailnet quand elle est fermée. Eyrie ne le lance pas : elle
// s'y rattache, par un tube nommé au nom du dossier du nœud, réservé au compte de l'utilisateur. Le tube parle comme
// l'entrée et la sortie du compagnon lancé par Eyrie : consignes de relais et {"arret":true} dans un sens, lignes
// d'état dans l'autre. Son client est vérifié (même compte Windows ; l'exécutable d'Eyrie ou du compagnon, rangé à
// côté du veilleur) ; un seul à la fois. Le client parti (Eyrie fermée, arrêtée ou plantée), plus rien n'est
// relayé, et le nœud reste sur le tailnet.

import (
	"bufio"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"strings"
	"sync/atomic"
	"time"

	"github.com/tailscale/go-winio"
	"golang.org/x/crypto/blake2s"
	"golang.org/x/sys/windows"
)

// pipeName : le tube du veilleur du nœud `dir`, `\\.\pipe\eyrie-tailnet-` suivi des 8 premiers octets (en
// hexadécimal) du BLAKE2s-256 de son dir_key.
func pipeName(dir string) string {
	sum := blake2s.Sum256([]byte(dirKey(dir)))
	return `\\.\pipe\eyrie-tailnet-` + hex.EncodeToString(sum[:8])
}

// userSID : le compte Windows du processus.
func userSID() (*windows.SID, error) {
	u, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return nil, fmt.Errorf("compte de l'utilisateur illisible : %w", err)
	}
	return u.User.Sid, nil
}

// listenControl ouvre le tube du nœud `dir` : réservé au compte de l'utilisateur (liste de contrôle protégée, sans
// héritage), fermé aux clients d'un autre poste. Échoue si le tube existe déjà : personne ne peut le créer avant le
// veilleur pour se faire passer pour lui.
func listenControl(dir string) (net.Listener, error) {
	me, err := userSID()
	if err != nil {
		return nil, err
	}
	return winio.ListenPipe(pipeName(dir), &winio.PipeConfig{
		SecurityDescriptor: "D:P(A;;GA;;;" + me.String() + ")",
		// Sans tampon, chaque ligne d'état attendrait qu'Eyrie la lise.
		InputBufferSize:  pipeBuffer,
		OutputBufferSize: pipeBuffer,
	})
}

// pipeHandle : le handle d'une connexion par tube.
func pipeHandle(c net.Conn) (windows.Handle, error) {
	f, ok := c.(interface{ Fd() uintptr })
	if !ok {
		return 0, errors.New("connexion sans handle")
	}
	return windows.Handle(f.Fd()), nil
}

// checkClient vérifie le processus au bout du tube `c` (voir checkProcess).
func checkClient(c net.Conn, allowed []string) error {
	h, err := pipeHandle(c)
	if err != nil {
		return err
	}
	var pid uint32
	if err := windows.GetNamedPipeClientProcessId(h, &pid); err != nil {
		return fmt.Errorf("processus du client illisible : %w", err)
	}
	return checkProcess(pid, allowed)
}

// checkProcess : le processus `pid` tourne-t-il sous le compte de l'utilisateur, depuis l'un des exécutables
// `allowed` ? Les fichiers sont comparés, pas les chemins (casse, noms courts).
func checkProcess(pid uint32, allowed []string) error {
	h, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return fmt.Errorf("processus %d : %w", pid, err)
	}
	defer windows.CloseHandle(h)
	var tok windows.Token
	if err := windows.OpenProcessToken(h, windows.TOKEN_QUERY, &tok); err != nil {
		return fmt.Errorf("compte du processus %d illisible : %w", pid, err)
	}
	defer tok.Close()
	user, err := tok.GetTokenUser()
	if err != nil {
		return fmt.Errorf("compte du processus %d illisible : %w", pid, err)
	}
	me, err := userSID()
	if err != nil {
		return err
	}
	if !user.User.Sid.Equals(me) {
		return fmt.Errorf("processus %d d'un autre compte (%s)", pid, user.User.Sid)
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
