//go:build windows

package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"golang.org/x/sys/windows"
)

func filled(b byte) []byte {
	return bytes.Repeat([]byte{b}, 32)
}

// Vecteurs partagés avec crates/suite-mobile/src/veille.rs : clé du PC 0x01…, clé publique 0x02…, défi 0x03….
func TestWakeVectorsMatchSuiteMobile(t *testing.T) {
	secret := deviceSecret(filled(1), filled(2))
	if got := hex.EncodeToString(secret); got != "12dd85dc64a26b78466d6343ca4f8ffdd0a74de58ab34ee9d4ce7b1650f86431" {
		t.Fatalf("secret : %s", got)
	}
	p := wakeProof(secret, filled(3), "appareil-1", wakeStart)
	if got := hex.EncodeToString(p); got != "ad8399515aa176a2c4cd1622d9763ab2df25c9133b54198cbd680b04c4310953" {
		t.Fatalf("preuve : %s", got)
	}
}

type launchCall struct {
	exe  string
	args []string
	env  []string
}

type wakeFixture struct {
	t      *testing.T
	mobile string
	w      *waker
	ln     net.Listener
	// Les connexions d'adresses du tailnet choisies (from).
	fake   *fakeListener
	mu     sync.Mutex
	clock  time.Time
	open   bool
	login  string
	runs   []launchCall
	unsafe error
	// La dernière Eyrie lancée est sortie ; combien ne sont plus suivies par le veilleur.
	exited   bool
	released int
	// Tant qu'il est ouvert, WhoIs attend (connexions en cours).
	hold chan struct{}
}

// fakeChild : une Eyrie lancée pour de faux, qui tourne tant que la fixture ne la dit pas sortie.
type fakeChild struct {
	f *wakeFixture
}

func (c fakeChild) running() bool {
	c.f.mu.Lock()
	defer c.f.mu.Unlock()
	return !c.f.exited
}

func (c fakeChild) release() {
	c.f.mu.Lock()
	defer c.f.mu.Unlock()
	c.f.released++
}

const essaiLogin = "aubin@exemple.org"

// Le compte Tailscale d'un autre appareil appairé (appareil-3).
const autreLogin = "autre@exemple.org"

// newWake : un veilleur d'essai sur la boucle locale, avec un appareil appairé « Pixel 8 » (clé publique 0x02…,
// compte essaiLogin), un autre appairé hors du tailnet (sans compte), un troisième d'un autre compte (clé 0x05…), et
// la clé du veilleur 0x01….
func newWake(t *testing.T) *wakeFixture {
	t.Helper()
	mobile := t.TempDir()
	devices := []map[string]any{
		{"id": "appareil-1", "name": "Pixel 8", "key": hex.EncodeToString(filled(2)), "login": essaiLogin, "paired_at": 1},
		{"id": "appareil-2", "name": "Essai", "key": hex.EncodeToString(filled(4)), "login": "", "paired_at": 1},
		{"id": "appareil-3", "name": "Tablette", "key": hex.EncodeToString(filled(5)), "login": autreLogin, "paired_at": 1},
	}
	b, _ := json.Marshal(devices)
	if err := os.WriteFile(filepath.Join(mobile, "appareils.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	f := &wakeFixture{t: t, mobile: mobile, clock: time.Unix(1_800_000_000, 0), login: essaiLogin}
	f.config(true)
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	f.ln = ln
	f.w = newWaker(mobile, `C:\Essai\eyrie.exe`, func(ctx context.Context, addr string) (identity, error) {
		f.mu.Lock()
		hold, login := f.hold, f.login
		f.mu.Unlock()
		if hold != nil {
			select {
			case <-hold:
			case <-ctx.Done():
				return identity{}, ctx.Err()
			}
		}
		return identity{Login: login, Address: addr}, nil
	}, t.Logf)
	f.w.now = func() time.Time {
		f.mu.Lock()
		defer f.mu.Unlock()
		return f.clock
	}
	f.w.isOpen = func(instance string) bool {
		f.mu.Lock()
		defer f.mu.Unlock()
		return f.open && instance == `Local\Eyrie-instance:c:/essai`
	}
	f.w.check = func(string) error {
		f.mu.Lock()
		defer f.mu.Unlock()
		return f.unsafe
	}
	f.w.launch = func(exe string, args, env []string) (child, error) {
		f.mu.Lock()
		defer f.mu.Unlock()
		f.runs = append(f.runs, launchCall{exe, args, env})
		f.exited = false
		return fakeChild{f}, nil
	}
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	go f.w.serve(ctx, ln)
	f.fake = &fakeListener{conns: make(chan net.Conn), closed: make(chan struct{})}
	go f.w.serve(ctx, f.fake)
	return f
}

// fakeListener : des connexions d'essai (net.Pipe), chacune d'une adresse du tailnet choisie.
type fakeListener struct {
	conns  chan net.Conn
	closed chan struct{}
	once   sync.Once
}

func (l *fakeListener) Accept() (net.Conn, error) {
	select {
	case c := <-l.conns:
		return c, nil
	case <-l.closed:
		return nil, net.ErrClosed
	}
}

func (l *fakeListener) Close() error {
	l.once.Do(func() { close(l.closed) })
	return nil
}

func (l *fakeListener) Addr() net.Addr {
	return &net.TCPAddr{IP: net.IPv4(100, 64, 0, 1), Port: wakePort}
}

// remoteConn : le bout du veilleur d'une connexion venue de `remote`.
type remoteConn struct {
	net.Conn
	remote net.Addr
}

func (c remoteConn) RemoteAddr() net.Addr {
	return c.remote
}

// from ouvre une connexion au veilleur depuis l'adresse du tailnet `addr` (« 100.64.0.7:41000 »).
func (f *wakeFixture) from(addr string) net.Conn {
	f.t.Helper()
	server, client := net.Pipe()
	f.t.Cleanup(func() { client.Close() })
	f.fake.conns <- remoteConn{server, net.TCPAddrFromAddrPort(netip.MustParseAddrPort(addr))}
	client.SetDeadline(time.Now().Add(10 * time.Second))
	return client
}

// closedAtOnce : le veilleur a-t-il fermé `c` sans rien dire (3 s au plus) ?
func closedAtOnce(c net.Conn) error {
	c.SetReadDeadline(time.Now().Add(3 * time.Second))
	n, err := c.Read(make([]byte, 1))
	if n == 0 && errors.Is(err, io.EOF) {
		return nil
	}
	return fmt.Errorf("lu %d octet(s), %v", n, err)
}

// config écrit mobile\veille.dpapi comme Eyrie.
func (f *wakeFixture) config(active bool) {
	f.t.Helper()
	raw, _ := json.Marshal(map[string]any{
		"v": 1, "actif": active, "cle": hex.EncodeToString(filled(1)),
		"instance": `Local\Eyrie-instance:c:/essai`, "reduite": true,
	})
	sealed, err := seal(raw, wakeEntropy, "essai")
	if err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.mobile, "veille.dpapi"), sealed, 0o600); err != nil {
		f.t.Fatal(err)
	}
}

func (f *wakeFixture) set(change func(f *wakeFixture)) {
	f.mu.Lock()
	defer f.mu.Unlock()
	change(f)
}

func (f *wakeFixture) launches() []launchCall {
	f.mu.Lock()
	defer f.mu.Unlock()
	return slices.Clone(f.runs)
}

type exchange struct {
	hello    wakeHello
	rawHello string
	reply    wakeReply
}

// ask fait une demande comme le téléphone : l'appareil `device`, avec la preuve que rend `proof` pour le défi reçu.
func (f *wakeFixture) ask(device, request string, proof func(challenge []byte) []byte) (exchange, error) {
	var x exchange
	c, err := net.Dial("tcp", f.ln.Addr().String())
	if err != nil {
		return x, err
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(10 * time.Second))
	rd := bufio.NewReader(c)
	line, err := rd.ReadString('\n')
	if err != nil {
		return x, fmt.Errorf("défi : %w", err)
	}
	x.rawHello = strings.TrimSpace(line)
	if err := json.Unmarshal([]byte(line), &x.hello); err != nil {
		return x, err
	}
	challenge, err := hex.DecodeString(x.hello.Challenge)
	if err != nil || len(challenge) != 32 {
		return x, fmt.Errorf("défi invalide : %q", x.hello.Challenge)
	}
	req, _ := json.Marshal(wakeRequest{Device: device, Request: request, Proof: hex.EncodeToString(proof(challenge))})
	if _, err := c.Write(append(req, '\n')); err != nil {
		return x, err
	}
	line, err = rd.ReadString('\n')
	if err != nil {
		return x, fmt.Errorf("réponse : %w", err)
	}
	return x, json.Unmarshal([]byte(line), &x.reply)
}

// goodProof : la preuve du téléphone « Pixel 8 », qui a reçu son secret dans Welcome.
func goodProof(challenge []byte) []byte {
	return wakeProof(deviceSecret(filled(1), filled(2)), challenge, "appareil-1", wakeStart)
}

func badProof([]byte) []byte {
	return filled(9)
}

func (f *wakeFixture) start() exchange {
	f.t.Helper()
	x, err := f.ask("appareil-1", wakeStart, goodProof)
	if err != nil {
		f.t.Fatal(err)
	}
	return x
}

func (f *wakeFixture) journal() []journalLine {
	f.t.Helper()
	return readJournal(f.t, f.w.mobile)
}

// readJournal : les lignes du journal du pont `mobile`.
func readJournal(t *testing.T, mobile string) []journalLine {
	t.Helper()
	raw, _ := os.ReadFile(filepath.Join(mobile, "journal.jsonl"))
	var out []journalLine
	for _, l := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		if l == "" {
			continue
		}
		var j journalLine
		if err := json.Unmarshal([]byte(l), &j); err != nil {
			t.Fatalf("ligne du journal : %q %v", l, err)
		}
		out = append(out, j)
	}
	return out
}

// Un téléphone appairé démarre Eyrie : réduite, avec son nom, journalisé ; tant qu'elle démarre ou une fois
// ouverte, rien n'est relancé.
func TestAPairedPhoneStartsEyrie(t *testing.T) {
	f := newWake(t)
	x := f.start()
	if x.hello.Eyrie != eyrieClosed || !x.reply.OK || x.reply.State != eyrieStarting {
		t.Fatalf("échange : %+v", x)
	}
	if !strings.HasPrefix(x.rawHello, `{"v":1,"defi":"`) || !strings.HasSuffix(x.rawHello, `","eyrie":"fermee"}`) {
		t.Fatalf("défi : %s", x.rawHello)
	}
	runs := f.launches()
	if len(runs) != 1 || runs[0].exe != `C:\Essai\eyrie.exe` || !slices.Equal(runs[0].args, []string{"--reduite"}) ||
		!slices.Equal(runs[0].env, []string{"EYRIE_DEMARRE_PAR=Pixel 8"}) {
		t.Fatalf("lancement : %+v", runs)
	}
	j := f.journal()
	if len(j) != 1 || j[0].Kind != "veille" || j[0].Device != "Pixel 8" || j[0].At != 1_800_000_000 ||
		!strings.Contains(j[0].Text, "démarrée") {
		t.Fatalf("journal : %+v", j)
	}

	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(10 * time.Second) })
	x = f.start()
	if x.hello.Eyrie != eyrieStarting || !x.reply.OK || len(f.launches()) != 1 {
		t.Fatalf("en démarrage : %+v, %d lancements", x, len(f.launches()))
	}
	f.set(func(f *wakeFixture) { f.open = true })
	x = f.start()
	if x.hello.Eyrie != eyrieOpen || x.reply.OK || !strings.Contains(x.reply.Reason, "déjà ouverte") ||
		len(f.launches()) != 1 {
		t.Fatalf("ouverte : %+v, %d lancements", x, len(f.launches()))
	}
}

// Eyrie ouverte puis refermée dans la minute n'est plus « en démarrage » : le téléphone peut la redémarrer, une fois
// passées les 30 s entre deux démarrages.
func TestEyrieOpenedThenClosedCanBeStartedAgain(t *testing.T) {
	f := newWake(t)
	f.start()
	f.set(func(f *wakeFixture) {
		f.clock = f.clock.Add(5 * time.Second)
		f.open = true
	})
	if x := f.start(); x.hello.Eyrie != eyrieOpen {
		t.Fatalf("ouverte : %+v", x)
	}
	f.set(func(f *wakeFixture) {
		f.clock = f.clock.Add(5 * time.Second)
		f.open = false
	})
	x := f.start()
	if x.hello.Eyrie != eyrieClosed || x.reply.OK || !strings.Contains(x.reply.Reason, "réessayez dans 20 s") {
		t.Fatalf("refermée après 10 s : %+v", x)
	}
	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(20 * time.Second) })
	if x := f.start(); x.hello.Eyrie != eyrieClosed || !x.reply.OK || len(f.launches()) != 2 {
		t.Fatalf("après 30 s : %+v, %d lancements", x, len(f.launches()))
	}
	f.set(func(f *wakeFixture) {
		if f.released != 1 {
			t.Errorf("Eyrie ouverte encore suivie : %d", f.released)
		}
	})
}

// Une Eyrie lancée qui sort aussitôt (plantage au démarrage) n'est plus « en démarrage ».
func TestALaunchedEyrieThatExitsIsClosed(t *testing.T) {
	f := newWake(t)
	f.start()
	f.set(func(f *wakeFixture) {
		f.clock = f.clock.Add(31 * time.Second)
		f.exited = true
	})
	x := f.start()
	if x.hello.Eyrie != eyrieClosed || !x.reply.OK || len(f.launches()) != 2 {
		t.Fatalf("sortie : %+v, %d lancements", x, len(f.launches()))
	}
	// La seconde, toujours là, démarre encore ; au bout d'une minute, elle n'est plus suivie.
	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(10 * time.Second) })
	if x := f.start(); x.hello.Eyrie != eyrieStarting || len(f.launches()) != 2 {
		t.Fatalf("en démarrage : %+v", x)
	}
	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(startingFor) })
	if x := f.start(); x.hello.Eyrie != eyrieClosed || len(f.launches()) != 3 {
		t.Fatalf("une minute après : %+v", x)
	}
	f.set(func(f *wakeFixture) {
		if f.released != 2 {
			t.Errorf("Eyrie suivies : %d relâchées", f.released)
		}
	})
}

// Un appareil d'un autre compte Tailscale, ou un compte vide, ne reçoit rien, pas même un défi.
func TestAStrangerGetsNothing(t *testing.T) {
	f := newWake(t)
	for _, login := range []string{"intrus@exemple.org", ""} {
		f.set(func(f *wakeFixture) { f.login = login })
		if _, err := f.ask("appareil-1", wakeStart, goodProof); err == nil || !strings.Contains(err.Error(), "défi") {
			t.Fatalf("compte %q : %v", login, err)
		}
	}
	if len(f.launches()) != 0 || len(f.journal()) != 0 {
		t.Fatal("un inconnu a laissé une trace")
	}
}

// L'identifiant d'une preuve fausse n'est écrit au journal du PC que s'il a la forme de ceux d'Eyrie (12 chiffres
// hexadécimaux), « illisible » sinon : un appareil du compte n'y ajoute pas de lignes de son cru, que la page
// Téléphones montrerait comme les autres.
func TestAWrongProofOnlyJournalsAWellFormedDeviceId(t *testing.T) {
	f := newWake(t)
	forged := "x\n2 min · Pixel 8 · veille · Eyrie démarrée depuis le téléphone"
	for _, device := range []string{forged, "0123456789ab"} {
		if x, err := f.ask(device, wakeStart, badProof); err != nil || x.reply.Reason != "preuve refusée" {
			t.Fatalf("%q : %+v %v", device, x, err)
		}
	}
	j := f.journal()
	if len(j) != 2 || strings.ContainsAny(j[0].Text, "\r\n") || strings.Contains(j[0].Text, "Pixel") ||
		!strings.Contains(j[0].Text, "appareil « illisible »") {
		t.Fatalf("journal : %+v", j)
	}
	if !strings.Contains(j[1].Text, "appareil « 0123456789ab »") {
		t.Fatalf("identifiant bien formé : %+v", j[1])
	}
}

// Cinq preuves fausses bloquent l'appareil un quart d'heure, même avec la bonne preuve ensuite.
func TestWrongProofsBlockThePeer(t *testing.T) {
	f := newWake(t)
	for i := range maxWrongProofs {
		device := "appareil-1"
		if i == 0 {
			device = "inconnu"
		}
		x, err := f.ask(device, wakeStart, badProof)
		if err != nil || x.reply.OK || x.reply.Reason != "preuve refusée" {
			t.Fatalf("preuve fausse %d : %+v %v", i, x, err)
		}
	}
	x := f.start()
	if x.reply.OK || !strings.Contains(x.reply.Reason, "15 min") || len(f.launches()) != 0 {
		t.Fatalf("bloqué : %+v", x)
	}
	if j := f.journal(); len(j) != maxWrongProofs || !strings.Contains(j[len(j)-1].Text, "bloqué") {
		t.Fatalf("journal : %+v", j)
	}
	// La preuve d'un autre appareil (secret d'une autre clé publique) ne vaut pas non plus.
	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(blockedFor) })
	other := func(challenge []byte) []byte {
		return wakeProof(deviceSecret(filled(1), filled(4)), challenge, "appareil-1", wakeStart)
	}
	if x, err := f.ask("appareil-1", wakeStart, other); err != nil || x.reply.OK {
		t.Fatalf("preuve d'un autre appareil : %+v %v", x, err)
	}
	if x := f.start(); !x.reply.OK || len(f.launches()) != 1 {
		t.Fatalf("débloqué : %+v", x)
	}
}

// Un appareil est lié à son compte Tailscale : sa preuve, valable, ne vaut pas depuis la connexion d'un autre compte
// appairé (un téléphone qui rejouerait l'identifiant et le secret d'un autre).
func TestADeviceIsBoundToItsAccount(t *testing.T) {
	f := newWake(t)
	tablet := func(challenge []byte) []byte {
		return wakeProof(deviceSecret(filled(1), filled(5)), challenge, "appareil-3", wakeStart)
	}
	x, err := f.ask("appareil-3", wakeStart, tablet)
	if err != nil || x.reply.OK || x.reply.Reason != "preuve refusée" || len(f.launches()) != 0 {
		t.Fatalf("depuis le compte %s : %+v %v", essaiLogin, x, err)
	}
	if j := f.journal(); len(j) != 1 || !strings.Contains(j[0].Text, essaiLogin) || j[0].Device != "" {
		t.Fatalf("journal : %+v", j)
	}
	f.set(func(f *wakeFixture) { f.login = autreLogin })
	if x, err := f.ask("appareil-3", wakeStart, tablet); err != nil || !x.reply.OK || len(f.launches()) != 1 {
		t.Fatalf("depuis son compte : %+v %v", x, err)
	}
}

// Accès depuis le téléphone coupé sur le PC : le défi le dit, et rien n'est lancé.
func TestAccessCutOnThePcRefuses(t *testing.T) {
	f := newWake(t)
	f.config(false)
	x := f.start()
	if x.hello.Eyrie != eyrieCut || x.reply.OK || !strings.Contains(x.reply.Reason, "coupé") || len(f.launches()) != 0 {
		t.Fatalf("accès coupé : %+v", x)
	}
	if j := f.journal(); len(j) != 1 || !strings.Contains(j[0].Text, "refusé") {
		t.Fatalf("journal : %+v", j)
	}
}

// rights écrit les droits de « Pixel 8 » dans appareils.json comme Eyrie (`raw`, du JSON), ou les retire ("").
func (f *wakeFixture) rights(raw string) {
	f.t.Helper()
	path := filepath.Join(f.mobile, "appareils.json")
	b, err := os.ReadFile(path)
	if err != nil {
		f.t.Fatal(err)
	}
	var devices []map[string]json.RawMessage
	if err := json.Unmarshal(b, &devices); err != nil {
		f.t.Fatal(err)
	}
	if raw == "" {
		delete(devices[0], "rights")
	} else {
		devices[0]["rights"] = json.RawMessage(raw)
	}
	b, _ = json.Marshal(devices)
	if err := os.WriteFile(path, b, 0o600); err != nil {
		f.t.Fatal(err)
	}
}

// Démarrer Eyrie est une action : un téléphone sans le droit « Agir » est refusé, même avec une preuve valable (une
// clé reçue avant que le droit lui soit retiré), sans que cela compte pour une preuve fausse. Sans droits écrits
// (appairé avant eux), il a « Agir », comme chez Eyrie ; des droits sans « act » lisible ne le donnent pas.
func TestAPhoneWithoutTheActRightCannotStartEyrie(t *testing.T) {
	f := newWake(t)
	for i, raw := range []string{
		`{"see":true,"act":false,"terminals":true,"files":true}`,
		`{"see":true,"terminals":true}`,
		`{"see":true,"act":"oui","terminals":true}`,
		`null`,
	} {
		f.rights(raw)
		x := f.start()
		if x.hello.Eyrie != eyrieClosed || x.reply.OK || x.reply.Reason != actRefused || len(f.launches()) != 0 {
			t.Fatalf("droits %s : %+v", raw, x)
		}
		if j := f.journal(); len(j) != i+1 || j[i].Device != "Pixel 8" || !strings.Contains(j[i].Text, "« Agir »") {
			t.Fatalf("journal : %+v", j)
		}
	}
	f.w.mu.Lock()
	s := f.w.wrong["127.0.0.1"]
	f.w.mu.Unlock()
	if s != nil {
		t.Fatalf("compté pour une preuve fausse : %+v", s)
	}
	f.rights(`{"see":true,"act":true,"terminals":false,"files":false}`)
	if x := f.start(); !x.reply.OK || len(f.launches()) != 1 {
		t.Fatalf("avec « Agir » : %+v", x)
	}
	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(time.Hour) })
	f.rights("")
	if x := f.start(); !x.reply.OK || len(f.launches()) != 2 {
		t.Fatalf("sans droits écrits : %+v", x)
	}
}

// Six démarrages par heure au plus ; une demande d'un autre type est refusée.
func TestStartsAreRateLimited(t *testing.T) {
	f := newWake(t)
	first := f.w.now()
	for i := range startsPerHour {
		if x := f.start(); !x.reply.OK {
			t.Fatalf("démarrage %d : %+v", i, x)
		}
		f.set(func(f *wakeFixture) { f.clock = f.clock.Add(startingFor + time.Second) })
	}
	x := f.start()
	if x.reply.OK || !strings.Contains(x.reply.Reason, "trop de démarrages") || len(f.launches()) != startsPerHour {
		t.Fatalf("septième : %+v", x)
	}
	f.set(func(f *wakeFixture) { f.clock = first.Add(time.Hour) })
	if x := f.start(); !x.reply.OK {
		t.Fatalf("une heure après : %+v", x)
	}
	stop := func(challenge []byte) []byte {
		return wakeProof(deviceSecret(filled(1), filled(2)), challenge, "appareil-1", "arreter")
	}
	f.set(func(f *wakeFixture) { f.clock = f.clock.Add(time.Hour) })
	if x, err := f.ask("appareil-1", "arreter", stop); err != nil || x.reply.OK ||
		!strings.Contains(x.reply.Reason, "inconnue") {
		t.Fatalf("autre demande : %+v %v", x, err)
	}
}

func TestOneStartEveryThirtySeconds(t *testing.T) {
	w := &waker{}
	now := time.Unix(1_800_000_000, 0)
	w.starts = []time.Time{now.Add(-10 * time.Second)}
	if wait := w.startWait(now); wait != 20*time.Second {
		t.Fatalf("attente : %v", wait)
	}
	w.starts = []time.Time{now.Add(-2 * time.Hour), now.Add(-31 * time.Second)}
	if wait := w.startWait(now); wait != 0 || len(w.starts) != 1 {
		t.Fatalf("attente : %v, %d gardés", wait, len(w.starts))
	}
}

// Un eyrie.exe modifiable par un autre compte n'est pas lancé ; absent ou de droits illisibles non plus, mais le
// téléphone ne l'apprend pas sous le même nom.
func TestAnUnsafeEyrieIsNotLaunched(t *testing.T) {
	f := newWake(t)
	for i, c := range []struct {
		err          error
		reason, logs string
	}{
		{&unsafePlace{path: `C:\Outils`, who: "S-1-5-11"}, "pas à l'abri", "S-1-5-11"},
		{fmt.Errorf(`C:\Essai\eyrie.exe introuvable : %w`, fs.ErrNotExist), "introuvable", "introuvable"},
		{errors.New(`droits de C:\Essai\eyrie.exe illisibles : refusé`), "illisible", "illisibles"},
	} {
		f.set(func(f *wakeFixture) {
			f.unsafe = c.err
			f.clock = f.clock.Add(time.Hour)
		})
		x := f.start()
		if x.reply.OK || !strings.Contains(x.reply.Reason, c.reason) || len(f.launches()) != 0 {
			t.Fatalf("%v : %+v", c.err, x)
		}
		if j := f.journal(); len(j) != i+1 || !strings.Contains(j[i].Text, c.logs) {
			t.Fatalf("journal : %+v", j)
		}
	}
}

// Une ligne trop longue ferme la connexion.
func TestOverlongLinesAreDropped(t *testing.T) {
	f := newWake(t)
	c, err := net.Dial("tcp", f.ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(10 * time.Second))
	rd := bufio.NewReader(c)
	if _, err := rd.ReadString('\n'); err != nil {
		t.Fatal(err)
	}
	_, _ = c.Write(bytes.Repeat([]byte("x"), 2*maxWakeLine))
	if line, err := rd.ReadString('\n'); err == nil {
		t.Fatalf("réponse à une ligne trop longue : %q", line)
	}
}

// Une connexion par adresse du tailnet : la nouvelle remplace la précédente (le téléphone qui demande le démarrage
// juste après une sonde), et un appareil du compte qui en ouvre à la chaîne, sans jamais rien demander, ne prend pas
// la place des autres. Au-delà de huit adresses, les suivantes sont fermées tout de suite.
func TestOneConnectionPerTailnetAddress(t *testing.T) {
	f := newWake(t)
	hold := make(chan struct{})
	f.set(func(f *wakeFixture) { f.hold = hold })
	first := f.from("100.64.0.7:41000")
	eventually(t, "première connexion", func() bool { return f.w.connections() == 1 })
	again := f.from("100.64.0.7:41001")
	if err := closedAtOnce(first); err != nil {
		t.Fatalf("connexion remplacée : %v", err)
	}
	if n := f.w.connections(); n != 1 {
		t.Fatalf("connexions : %d", n)
	}
	// Le même appareil en IPv6 est une autre adresse ; puis six autres appareils.
	held := []net.Conn{again, f.from("[fd7a:115c:a1e0::7]:41002")}
	for i := range maxWakeConns - 2 {
		held = append(held, f.from(fmt.Sprintf("100.64.0.%d:41000", 10+i)))
	}
	eventually(t, "connexions en cours", func() bool { return f.w.connections() == maxWakeConns })
	if err := closedAtOnce(f.from("100.64.0.99:41000")); err != nil {
		t.Fatalf("neuvième adresse : %v", err)
	}
	// Plein, une adresse déjà là remplace encore la sienne.
	held[0] = f.from("100.64.0.7:41003")
	if err := closedAtOnce(again); err != nil {
		t.Fatalf("connexion remplacée, plein : %v", err)
	}
	f.set(func(f *wakeFixture) { f.hold = nil })
	close(hold)
	for i, c := range held {
		if line, err := bufio.NewReader(c).ReadString('\n'); err != nil || !strings.Contains(line, `"defi"`) {
			t.Fatalf("connexion tenue %d : %q %v", i, line, err)
		}
		c.Close()
	}
	eventually(t, "connexions finies", func() bool { return f.w.connections() == 0 })
}

// Le réglage du veilleur n'est lu que s'il vient d'Eyrie (DPAPI, entropie eyrie/veille) et qu'il est complet.
func TestTheWakeConfigMustBeEyries(t *testing.T) {
	dir := t.TempDir()
	write := func(sealed []byte) {
		if err := os.WriteFile(filepath.Join(dir, "veille.dpapi"), sealed, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	good := map[string]any{"v": 1, "actif": true, "cle": hex.EncodeToString(filled(1)),
		"instance": `Local\Eyrie-instance:c:/essai`, "reduite": false}
	sealWith := func(m map[string]any, ent []byte) []byte {
		raw, _ := json.Marshal(m)
		s, err := seal(raw, ent, "essai")
		if err != nil {
			t.Fatal(err)
		}
		return s
	}
	write(sealWith(good, wakeEntropy))
	cfg, key, err := readWakeConfig(dir)
	if err != nil || !cfg.Active || cfg.Minimized || !bytes.Equal(key, filled(1)) {
		t.Fatalf("relu : %+v %x %v", cfg, key, err)
	}
	for what, m := range map[string]map[string]any{
		"version":  {"v": 2, "actif": true, "cle": good["cle"], "instance": good["instance"]},
		"clé":      {"v": 1, "actif": true, "cle": "abcd", "instance": good["instance"]},
		"instance": {"v": 1, "actif": true, "cle": good["cle"], "instance": `Global\autre`},
	} {
		write(sealWith(m, wakeEntropy))
		if _, _, err := readWakeConfig(dir); err == nil {
			t.Errorf("%s : accepté", what)
		}
	}
	write(sealWith(good, []byte("autre entropie")))
	if _, _, err := readWakeConfig(dir); err == nil {
		t.Error("autre entropie : accepté")
	}
}

// L'état « ouverte » se lit sur le mutex d'instance d'Eyrie.
func TestEyrieIsOpenWhileItHoldsItsInstance(t *testing.T) {
	name := fmt.Sprintf(`Local\Eyrie-instance:essai-%d`, os.Getpid())
	if instanceOpen(name) {
		t.Fatal("ouverte avant d'exister")
	}
	p, _ := windows.UTF16PtrFromString(name)
	h, err := windows.CreateMutex(nil, false, p)
	if err != nil {
		t.Fatal(err)
	}
	if !instanceOpen(name) {
		t.Fatal("fermée alors qu'elle tient son instance")
	}
	windows.CloseHandle(h)
	if instanceOpen(name) {
		t.Fatal("ouverte après sa fermeture")
	}
}

// Le journal du pont passe à journal.1.jsonl au-delà d'un mégaoctet, comme chez Eyrie.
func TestTheJournalRollsOverLikeEyries(t *testing.T) {
	w := &waker{mobile: t.TempDir(), logf: t.Logf}
	path := filepath.Join(w.mobile, "journal.jsonl")
	if err := os.WriteFile(path, bytes.Repeat([]byte("x"), journalMax+1), 0o600); err != nil {
		t.Fatal(err)
	}
	w.journal("Pixel <8>", "Eyrie démarrée", time.Unix(5, 0))
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(raw) != `{"at":5,"device":"Pixel <8>","kind":"veille","text":"Eyrie démarrée"}`+"\n" {
		t.Fatalf("journal : %q", raw)
	}
	if st, err := os.Stat(filepath.Join(w.mobile, "journal.1.jsonl")); err != nil || st.Size() != journalMax+1 {
		t.Fatalf("ancien journal : %v", err)
	}
}

// Les fichiers d'Eyrie sont lus sans gêner leur remplacement : tant que le veilleur les tient ouverts, un autre
// processus peut encore les ouvrir pour les supprimer, ce que fait le renommage d'Eyrie (std::fs::rename) ; pas avec
// os.Open.
func TestEyriesFilesAreReadWithoutBlockingItsSaves(t *testing.T) {
	path := filepath.Join(t.TempDir(), "appareils.json")
	if err := os.WriteFile(path, []byte("[]"), 0o600); err != nil {
		t.Fatal(err)
	}
	deletable := func() error {
		p, _ := windows.UTF16PtrFromString(path)
		h, err := windows.CreateFile(p, windows.DELETE, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|
			windows.FILE_SHARE_DELETE, nil, windows.OPEN_EXISTING, windows.FILE_ATTRIBUTE_NORMAL, 0)
		if err == nil {
			windows.CloseHandle(h)
		}
		return err
	}
	held, err := openShared(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := deletable(); err != nil {
		t.Fatalf("tenu par le veilleur : %v", err)
	}
	held.Close()
	plain, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer plain.Close()
	if err := deletable(); !errors.Is(err, windows.ERROR_SHARING_VIOLATION) {
		t.Fatalf("os.Open ne gêne plus le renommage : %v", err)
	}
	if raw, err := readShared(path); err != nil || string(raw) != "[]" {
		t.Fatalf("relu : %q %v", raw, err)
	}
	if _, err := readShared(path + ".absent"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("absent : %v", err)
	}
}

// Le nom d'un appareil perd ses caractères de contrôle et de format (sens d'écriture), mais garde le liant des émojis.
func TestADeviceNameIsCleanedForTheEnvironment(t *testing.T) {
	if got := envText("Pixel\x00 8\n" + strings.Repeat("é", 80)); got != "Pixel 8"+strings.Repeat("é", 57) {
		t.Fatalf("%q", got)
	}
	if got := envText("‮Pixel⁦ 8​ 👩‍💻؜"); got != "Pixel 8 👩‍💻" {
		t.Fatalf("%q", got)
	}
}
