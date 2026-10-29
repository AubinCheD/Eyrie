//go:build windows

package main

import (
	"bufio"
	"context"
	"encoding/hex"
	"encoding/json"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/tailscale/go-winio"
	"golang.org/x/crypto/blake2s"
	"golang.org/x/sys/windows"
)

// Le nom du tube est tiré au hasard à chaque démarrage, et écrit dans le dossier du nœud, où Eyrie le lit.
func TestThePipeNameIsDrawnAtEachStartAndWrittenForEyrie(t *testing.T) {
	dir := t.TempDir()
	var names []string
	for range 2 {
		ln, err := listenControl(dir)
		if err != nil {
			t.Fatal(err)
		}
		name := controlPipe(dir)
		raw, _ := os.ReadFile(filepath.Join(dir, pipeFile))
		if !strings.HasPrefix(name, `\\.\pipe\eyrie-tailnet-`) || len(name) != len(`\\.\pipe\eyrie-tailnet-`)+32 ||
			string(raw) != name+"\n" {
			t.Fatalf("nom : %q, %s : %q", name, pipeFile, raw)
		}
		go func() {
			if c, err := ln.Accept(); err == nil {
				c.Close()
			}
		}()
		timeout := 5 * time.Second
		c, err := winio.DialPipe(name, &timeout)
		if err != nil {
			t.Fatalf("tube %s : %v", name, err)
		}
		c.Close()
		ln.Close()
		names = append(names, name)
	}
	if names[0] == names[1] {
		t.Fatalf("même nom deux fois : %s", names[0])
	}
	// Seul un nom de tube de veilleur est lu (ni un tube d'un autre poste, ni autre chose).
	suffix := strings.Repeat("0123456789abcdef", 2)
	for _, bad := range []string{`\\serveur\pipe\eyrie-tailnet-` + suffix, `\\.\pipe\autre-` + suffix,
		`\\.\pipe\eyrie-tailnet-` + strings.ToUpper(suffix), `\\.\pipe\eyrie-tailnet-` + suffix[:16], ""} {
		if err := os.WriteFile(filepath.Join(dir, pipeFile), []byte(bad+"\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		if got := controlPipe(dir); got != "" {
			t.Errorf("%q lu : %q", bad, got)
		}
	}
	os.Remove(filepath.Join(dir, pipeFile))
	if got := controlPipe(dir); got != "" {
		t.Errorf("sans fichier : %q", got)
	}
}

// Les tubes sont communs à tout le poste : un autre compte qui prend le premier le nom qu'il devine (celui que tirait
// le veilleur du dossier du nœud) n'empêche plus le veilleur de démarrer ; un nom tiré déjà pris, il en tire un autre.
func TestAnotherAccountCannotTakeThePipeFirst(t *testing.T) {
	dir := t.TempDir()
	sum := blake2s.Sum256([]byte(dirKey(dir)))
	guessed := `\\.\pipe\eyrie-tailnet-` + hex.EncodeToString(sum[:8])
	squat, err := winio.ListenPipe(guessed, &winio.PipeConfig{SecurityDescriptor: "D:P(A;;GA;;;SY)"})
	if err != nil {
		t.Fatal(err)
	}
	defer squat.Close()
	ln, err := listenControl(dir)
	if err != nil {
		t.Fatalf("nom deviné pris : %v", err)
	}
	ln.Close()

	taken := `\\.\pipe\eyrie-tailnet-` + strings.Repeat("ab", 16)
	squat2, err := winio.ListenPipe(taken, &winio.PipeConfig{SecurityDescriptor: "D:P(A;;GA;;;SY)"})
	if err != nil {
		t.Fatal(err)
	}
	defer squat2.Close()
	draw := newPipeName
	t.Cleanup(func() { newPipeName = draw })
	drawn := 0
	newPipeName = func() (string, error) {
		drawn++
		if drawn == 1 {
			return taken, nil
		}
		return draw()
	}
	ln, err = listenControl(dir)
	if err != nil {
		t.Fatalf("nom tiré pris : %v", err)
	}
	defer ln.Close()
	if name := controlPipe(dir); drawn != 2 || name == taken || name == "" {
		t.Fatalf("%d tirages, nom %q", drawn, name)
	}
}

type standbyFixture struct {
	t       *testing.T
	dir     string
	relay   *relay
	tube    *tube
	stopped chan struct{}
	cancel  context.CancelFunc
}

// newStandby ouvre le tube d'un veilleur sur un dossier d'essai, qui accepte les clients lancés depuis `allowed`.
func newStandby(t *testing.T, allowed ...string) *standbyFixture {
	t.Helper()
	f := &standbyFixture{t: t, dir: t.TempDir(), relay: &relay{}, stopped: make(chan struct{})}
	ln, err := listenControl(f.dir)
	if err != nil {
		t.Fatal(err)
	}
	saved := status
	status = &statusLines{}
	ctx, cancel := context.WithCancel(context.Background())
	f.cancel = cancel
	t.Cleanup(func() {
		cancel()
		status = saved
	})
	f.tube = &tube{
		relay: f.relay,
		check: func(c net.Conn) error { return checkClient(c, allowed) },
		stop:  func() { close(f.stopped) },
		logf:  t.Logf,
	}
	go f.tube.serve(ctx, ln)
	return f
}

type pipeClient struct {
	net.Conn
	rd *bufio.Reader
}

func (f *standbyFixture) dial() *pipeClient {
	f.t.Helper()
	timeout := 5 * time.Second
	c, err := winio.DialPipe(controlPipe(f.dir), &timeout)
	if err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { c.Close() })
	c.SetDeadline(time.Now().Add(10 * time.Second))
	return &pipeClient{Conn: c, rd: bufio.NewReader(c)}
}

func (c *pipeClient) send(t *testing.T, line string) {
	t.Helper()
	if _, err := c.Write([]byte(line + "\n")); err != nil {
		t.Fatal(err)
	}
}

func (c *pipeClient) next(t *testing.T) map[string]string {
	t.Helper()
	line, err := c.rd.ReadBytes('\n')
	if err != nil {
		t.Fatalf("ligne attendue : %v", err)
	}
	var m map[string]string
	if err := json.Unmarshal(line, &m); err != nil {
		t.Fatalf("%q : %v", line, err)
	}
	return m
}

func eventually(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatal(what)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func self(t *testing.T) string {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	return exe
}

// Eyrie se rattache : elle reçoit le dernier état du nœud, puis les suivants ; ses consignes de relais
// s'appliquent ; partie, le relais est coupé, et une autre se rattache.
func TestEyrieAttachesThroughThePipe(t *testing.T) {
	f := newStandby(t, self(t))
	emit(map[string]string{"state": "running", "name": "suite-pc.exemple.ts.net", "ip": "100.1.2.3"})

	eyrie := f.dial()
	if got := eyrie.next(t); got["state"] != "running" || got["name"] != "suite-pc.exemple.ts.net" {
		t.Fatalf("dernier état : %v", got)
	}
	eyrie.send(t, `{"relais":"127.0.0.1:5000","jeton":"abc"}`)
	eventually(t, "consigne de relais ignorée", func() bool { return f.relay.current.Load() != nil })
	fail("essai")
	if got := eyrie.next(t); got["state"] != "error" || got["message"] != "essai" {
		t.Fatalf("état suivant : %v", got)
	}

	eyrie.Close()
	eventually(t, "le relais reste ouvert après le départ d'Eyrie", func() bool { return f.relay.current.Load() == nil })
	again := f.dial()
	if got := again.next(t); got["state"] != "error" {
		t.Fatalf("dernier état : %v", got)
	}
	again.send(t, `{"relais":"127.0.0.1:5001","jeton":"def"}`)
	eventually(t, "seconde Eyrie ignorée", func() bool {
		cur := f.relay.current.Load()
		return cur != nil && cur.Addr == "127.0.0.1:5001"
	})
}

// Une Eyrie qui ne lit plus ses lignes d'état est lâchée (2 s au plus par ligne) : le veilleur n'en reste pas
// bloqué, le relais est coupé, et une autre se rattache.
func TestAnEyrieThatStopsReadingIsDropped(t *testing.T) {
	f := newStandby(t, self(t))
	stuck := f.dial()
	stuck.send(t, `{"relais":"127.0.0.1:5000","jeton":"abc"}`)
	eventually(t, "consigne de relais ignorée", func() bool { return f.relay.current.Load() != nil })
	big := map[string]string{"state": "error", "message": strings.Repeat("x", 4096)}
	start := time.Now()
	// Bien plus que les tampons du tube : une écriture finit par attendre, puis la séance est coupée.
	for i := 0; i < 500 && f.relay.current.Load() != nil; i++ {
		emit(big)
	}
	eventually(t, "Eyrie muette toujours rattachée", func() bool {
		return f.relay.current.Load() == nil && !f.tube.busy.Load()
	})
	if d := time.Since(start); d > 10*time.Second {
		t.Fatalf("le veilleur est resté bloqué %v", d)
	}
	again := f.dial()
	if got := again.next(t); got["state"] != "error" || len(got["message"]) != 4096 {
		t.Fatalf("dernier état : %v", got["state"])
	}
}

// Un seul client à la fois : un second est éconduit, sauf pour arrêter le veilleur.
func TestOneClientAtATimeButAnyCanStopIt(t *testing.T) {
	f := newStandby(t, self(t))
	eyrie := f.dial()
	eyrie.send(t, `{"relais":"127.0.0.1:5000","jeton":"abc"}`)
	eventually(t, "consigne de relais ignorée", func() bool { return f.relay.current.Load() != nil })

	other := f.dial()
	other.send(t, `{"relais":"127.0.0.1:6000","jeton":"zzz"}`)
	if got := other.next(t); got["state"] != "error" || !strings.Contains(got["message"], "déjà") {
		t.Fatalf("second client : %v", got)
	}
	if cur := f.relay.current.Load(); cur == nil || cur.Addr != "127.0.0.1:5000" {
		t.Fatalf("le second client a changé le relais : %v", cur)
	}

	stopper := f.dial()
	stopper.send(t, `{"arret":true}`)
	if got := stopper.next(t); got["state"] != "stopped" {
		t.Fatalf("réponse à l'arrêt : %v", got)
	}
	select {
	case <-f.stopped:
	case <-time.After(5 * time.Second):
		t.Fatal("le veilleur ne s'arrête pas")
	}
}

// L'Eyrie rattachée arrête le veilleur.
func TestTheAttachedEyrieStopsIt(t *testing.T) {
	f := newStandby(t, self(t))
	eyrie := f.dial()
	eyrie.send(t, `{"arret":true}`)
	if got := eyrie.next(t); got["state"] != "stopped" {
		t.Fatalf("réponse à l'arrêt : %v", got)
	}
	select {
	case <-f.stopped:
	case <-time.After(5 * time.Second):
		t.Fatal("le veilleur ne s'arrête pas")
	}
}

// Un processus lancé d'un autre exécutable n'est pas écouté : rien ne lui est dit, rien ne change.
func TestAnotherProgramIsRefused(t *testing.T) {
	f := newStandby(t, `C:\Windows\System32\notepad.exe`)
	emit(map[string]string{"state": "running", "name": "suite-pc", "ip": "100.1.2.3"})
	c := f.dial()
	// Le veilleur a peut-être déjà raccroché.
	_, _ = c.Write([]byte(`{"relais":"127.0.0.1:5000","jeton":"abc"}` + "\n"))
	if line, err := c.rd.ReadBytes('\n'); err == nil {
		t.Fatalf("un client refusé a reçu : %q", line)
	}
	if f.relay.current.Load() != nil {
		t.Fatal("un client refusé a changé le relais")
	}
}

// Le tube n'est ouvert qu'au compte de l'utilisateur, et un second serveur ne peut pas le reprendre.
func TestThePipeIsTheUsersAlone(t *testing.T) {
	f := newStandby(t, self(t))
	c := f.dial()
	h, err := pipeHandle(c.Conn)
	if err != nil {
		t.Fatal(err)
	}
	sd, err := windows.GetSecurityInfo(h, windows.SE_KERNEL_OBJECT, windows.DACL_SECURITY_INFORMATION)
	if err != nil {
		t.Fatal(err)
	}
	me, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	sddl := sd.String()
	if !strings.HasPrefix(sddl, "D:P(") || strings.Count(sddl, "(") != 1 || !strings.Contains(sddl, me.String()) {
		t.Fatalf("droits du tube : %s", sddl)
	}
	if ln, err := winio.ListenPipe(controlPipe(f.dir), nil); err == nil {
		ln.Close()
		t.Fatal("un second serveur a repris le tube")
	}
}

// Le contrôle d'un processus compare les fichiers : même exécutable sous un autre nom de chemin, autre exécutable ;
// et les comptes : celui lu sur le tube, ou à défaut celui du processus.
func TestAProcessIsRecognisedByItsFile(t *testing.T) {
	exe := self(t)
	pid := uint32(os.Getpid())
	if err := checkProcess(pid, nil, []string{strings.ToUpper(exe)}); err != nil {
		t.Fatal(err)
	}
	if err := checkProcess(pid, nil, []string{`C:\Windows\System32\notepad.exe`}); err == nil {
		t.Fatal("un autre exécutable est accepté")
	}
	me, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	if err := checkProcess(pid, me, []string{exe}); err != nil {
		t.Fatal(err)
	}
	system, _ := windows.StringToSid("S-1-5-18")
	if err := checkProcess(pid, system, []string{exe}); err == nil || !strings.Contains(err.Error(), "autre compte") {
		t.Fatalf("un autre compte est accepté : %v", err)
	}
}

// Le compte du client se lit sur le tube quand il se laisse identifier (Eyrie élevée comprise), pas s'il reste
// anonyme.
func TestTheClientIsIdentifiedOnThePipe(t *testing.T) {
	dir := t.TempDir()
	ln, err := listenControl(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	me, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	for _, level := range []winio.PipeImpLevel{winio.PipeImpLevelIdentification, winio.PipeImpLevelAnonymous} {
		dialed := make(chan net.Conn, 1)
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			c, err := winio.DialPipeAccessImpLevel(ctx, controlPipe(dir), windows.GENERIC_READ|windows.GENERIC_WRITE, level)
			if err != nil {
				t.Error(err)
			}
			dialed <- c
		}()
		c, err := ln.Accept()
		if err != nil {
			t.Fatal(err)
		}
		h, _ := pipeHandle(c)
		user, err := pipeClientUser(h)
		switch {
		case level == winio.PipeImpLevelIdentification && (err != nil || !user.Equals(me)):
			t.Errorf("client identifié : %v %v", user, err)
		case level == winio.PipeImpLevelAnonymous && err == nil:
			t.Errorf("client anonyme identifié : %v", user)
		}
		c.Close()
		if d := <-dialed; d != nil {
			d.Close()
		}
	}
}

// Eyrie se rattache quelle que soit la façon dont elle ouvre le tube : identifiée (dialControl), anonyme, ou par un
// CreateFile ordinaire (le compte est alors lu sur son processus).
func TestEyrieAttachesHoweverItOpensThePipe(t *testing.T) {
	f := newStandby(t, self(t))
	emit(map[string]string{"state": "running", "name": "suite-pc", "ip": "100.1.2.3"})
	attach := func(how string, c io.ReadWriteCloser) {
		t.Helper()
		defer c.Close()
		line, err := bufio.NewReader(c).ReadBytes('\n')
		if err != nil || !strings.Contains(string(line), `"running"`) {
			t.Fatalf("%s : %q %v", how, line, err)
		}
	}
	c, err := dialControl(controlPipe(f.dir), 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	attach("identifiée", c)
	free := func() bool { return !f.tube.busy.Load() }
	eventually(t, "le veilleur garde le client précédent", free)
	attach("anonyme", f.dial().Conn)
	eventually(t, "le veilleur garde le client précédent", free)
	name, _ := windows.UTF16PtrFromString(controlPipe(f.dir))
	var h windows.Handle
	eventually(t, "tube injoignable par CreateFile", func() bool {
		h, err = windows.CreateFile(name, windows.GENERIC_READ|windows.GENERIC_WRITE, 0, nil, windows.OPEN_EXISTING, 0, 0)
		return err == nil
	})
	attach("CreateFile", os.NewFile(uintptr(h), "tube"))
}
