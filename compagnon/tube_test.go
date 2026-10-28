//go:build windows

package main

import (
	"bufio"
	"context"
	"encoding/json"
	"net"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/tailscale/go-winio"
	"golang.org/x/sys/windows"
)

// Le nom du tube ne dépend que du dir_key du dossier du nœud : Eyrie le retrouve (même vecteur côté Rust).
func TestThePipeIsNamedFromTheNodeKey(t *testing.T) {
	name := pipeName(`C:\Users\Exemple\AppData\Local\EyrieSuite\Eyrie\tailnet`)
	if name != `\\.\pipe\eyrie-tailnet-3590a409a3e09356` {
		t.Fatalf("nom : %s", name)
	}
	if pipeName(`c:/users/exemple/appdata/local/eyriesuite/eyrie/tailnet/`) != name {
		t.Fatal("le nom dépend de la casse ou de la barre finale")
	}
	// Hors ASCII aussi (dir_key de Rust, puis BLAKE2s).
	for dir, want := range map[string]string{
		`C:\Users\ΝΙΚΟΣ\AppData\Local\EyrieSuite\Eyrie\tailnet`: `\\.\pipe\eyrie-tailnet-c18cd2cf7b942306`,
		`C:\Users\İrem\AppData\Local\EyrieSuite\Eyrie\tailnet\`: `\\.\pipe\eyrie-tailnet-cddb792376a64c8a`,
	} {
		if got := pipeName(dir); got != want {
			t.Errorf("%s : %s, attendu %s", dir, got, want)
		}
	}
}

type standbyFixture struct {
	t       *testing.T
	dir     string
	relay   *relay
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
	tb := &tube{
		relay: f.relay,
		check: func(c net.Conn) error { return checkClient(c, allowed) },
		stop:  func() { close(f.stopped) },
		logf:  t.Logf,
	}
	go tb.serve(ctx, ln)
	return f
}

type pipeClient struct {
	net.Conn
	rd *bufio.Reader
}

func (f *standbyFixture) dial() *pipeClient {
	f.t.Helper()
	timeout := 5 * time.Second
	c, err := winio.DialPipe(pipeName(f.dir), &timeout)
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

// Le tube n'est ouvert qu'au compte de l'utilisateur, et un second veilleur ne peut pas le reprendre.
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
	if ln, err := listenControl(f.dir); err == nil {
		ln.Close()
		t.Fatal("un second veilleur a repris le tube")
	}
}

// Le contrôle d'un processus compare les fichiers : même exécutable sous un autre nom de chemin, autre exécutable.
func TestAProcessIsRecognisedByItsFile(t *testing.T) {
	exe := self(t)
	if err := checkProcess(uint32(os.Getpid()), []string{strings.ToUpper(exe)}); err != nil {
		t.Fatal(err)
	}
	if err := checkProcess(uint32(os.Getpid()), []string{`C:\Windows\System32\notepad.exe`}); err == nil {
		t.Fatal("un autre exécutable est accepté")
	}
}
