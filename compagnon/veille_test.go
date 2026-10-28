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
	"net"
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
	mu     sync.Mutex
	clock  time.Time
	open   bool
	login  string
	runs   []launchCall
	unsafe error
	// Tant qu'il est ouvert, WhoIs attend (connexions en cours).
	hold chan struct{}
}

const essaiLogin = "aubin@exemple.org"

// newWake : un veilleur d'essai sur la boucle locale, avec un appareil appairé « Pixel 8 » (clé publique 0x02…,
// compte essaiLogin), un autre appairé hors du tailnet (sans compte), et la clé du veilleur 0x01….
func newWake(t *testing.T) *wakeFixture {
	t.Helper()
	mobile := t.TempDir()
	devices := []map[string]any{
		{"id": "appareil-1", "name": "Pixel 8", "key": hex.EncodeToString(filled(2)), "login": essaiLogin, "paired_at": 1},
		{"id": "appareil-2", "name": "Essai", "key": hex.EncodeToString(filled(4)), "login": "", "paired_at": 1},
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
	f.w.launch = func(exe string, args, env []string) error {
		f.mu.Lock()
		defer f.mu.Unlock()
		f.runs = append(f.runs, launchCall{exe, args, env})
		return nil
	}
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	go f.w.serve(ctx, ln)
	return f
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
	raw, _ := os.ReadFile(filepath.Join(f.w.mobile, "journal.jsonl"))
	var out []journalLine
	for _, l := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		if l == "" {
			continue
		}
		var j journalLine
		if err := json.Unmarshal([]byte(l), &j); err != nil {
			f.t.Fatalf("ligne du journal : %q %v", l, err)
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

// Un eyrie.exe modifiable par un autre compte n'est pas lancé.
func TestAnUnsafeEyrieIsNotLaunched(t *testing.T) {
	f := newWake(t)
	f.set(func(f *wakeFixture) {
		f.unsafe = errors.New(`C:\Outils peut être modifié par un autre compte du poste (S-1-5-11)`)
	})
	x := f.start()
	if x.reply.OK || !strings.Contains(x.reply.Reason, "abri") || len(f.launches()) != 0 {
		t.Fatalf("emplacement : %+v", x)
	}
	if j := f.journal(); len(j) != 1 || !strings.Contains(j[0].Text, "S-1-5-11") {
		t.Fatalf("journal : %+v", j)
	}
}

// Une ligne trop longue ferme la connexion ; au-delà de quatre connexions, les suivantes sont fermées tout de suite.
func TestOverlongLinesAndExtraConnectionsAreDropped(t *testing.T) {
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

	hold := make(chan struct{})
	f.set(func(f *wakeFixture) { f.hold = hold })
	var held []net.Conn
	for range maxWakeConns {
		c, err := net.Dial("tcp", f.ln.Addr().String())
		if err != nil {
			t.Fatal(err)
		}
		defer c.Close()
		held = append(held, c)
	}
	eventually(t, "connexions en cours", func() bool { return f.w.conns.Load() == maxWakeConns })
	extra, err := net.Dial("tcp", f.ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer extra.Close()
	extra.SetDeadline(time.Now().Add(3 * time.Second))
	if _, err := extra.Read(make([]byte, 1)); err == nil || errors.Is(err, os.ErrDeadlineExceeded) {
		t.Fatalf("cinquième connexion : %v", err)
	}
	f.set(func(f *wakeFixture) { f.hold = nil })
	close(hold)
	for _, c := range held {
		c.SetDeadline(time.Now().Add(5 * time.Second))
		if _, err := bufio.NewReader(c).ReadString('\n'); err != nil {
			t.Fatalf("connexion tenue : %v", err)
		}
	}
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

func TestADeviceNameIsCleanedForTheEnvironment(t *testing.T) {
	if got := envText("Pixel\x00 8\n" + strings.Repeat("é", 80)); got != "Pixel 8"+strings.Repeat("é", 57) {
		t.Fatalf("%q", got)
	}
}
