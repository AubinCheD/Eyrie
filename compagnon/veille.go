//go:build windows

package main

// Démarrer Eyrie depuis le téléphone quand elle est fermée : le veilleur écoute le port 4748 du tailnet et y tient le
// défi-réponse de crates/suite-mobile/src/veille.rs, en lignes JSON (chacune terminée par "\n", d'un Kio au plus ; une
// demande sans fin de ligne dans les 5 s est abandonnée, sans réponse) :
//
//  1. veilleur → téléphone : {"v":1,"defi":"<64 hex>","eyrie":"fermee|demarrage|ouverte|acces-coupe"} ;
//  2. téléphone → veilleur : {"appareil":"<id>","demande":"demarrer","preuve":"<64 hex>"} ;
//  3. veilleur → téléphone : {"ok":true,"etat":"demarrage"} ou {"ok":false,"raison":"…"}.
//
// Seul un appareil appairé reçoit un défi : le compte Tailscale de la connexion (WhoIs) doit être celui d'un appareil
// de mobile\appareils.json ; les autres ne reçoivent rien. Seul un appareil qui a le droit « Agir » démarre Eyrie.
// Le veilleur relit à chaque demande les appareils et mobile\veille.dpapi (écrit par Eyrie, chiffré par DPAPI) :
// retirer un appareil, lui retirer « Agir » ou couper l'accès sur le PC vaut tout de suite. Il n'a pas la clé du PC
// (Noise) : il peut seulement démarrer Eyrie, et rien d'autre qu'eyrie.exe rangé à côté de lui.

import (
	"bufio"
	"context"
	"crypto/hmac"
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"hash"
	"io"
	"io/fs"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode"

	"golang.org/x/crypto/blake2s"
	"golang.org/x/sys/windows"
)

const (
	// Port du veilleur sur le tailnet (suite-mobile : veille::PORT).
	wakePort = 4748
	// Seule demande connue (veille::START).
	wakeStart = "demarrer"
	// Le refus d'un appareil sans le droit « Agir » (veilleur::ACT_REFUSED de crates/eyrie-mobile, qui le reconnaît à
	// ce texte et oublie sa clé).
	actRefused = "ce téléphone n'a pas le droit « Agir » sur le PC"

	// Une connexion : 5 s au plus, des lignes d'un Kio au plus, quatre à la fois au plus.
	wakeDeadline = 5 * time.Second
	maxWakeLine  = 1024
	maxWakeConns = 4

	// Cinq preuves fausses d'un même appareil du tailnet le bloquent un quart d'heure.
	maxWrongProofs = 5
	blockedFor     = 15 * time.Minute
	// Un démarrage toutes les 30 s, six par heure au plus.
	startEvery    = 30 * time.Second
	startsPerHour = 6
	// Une Eyrie lancée depuis moins d'une minute, pas encore ouverte et toujours là, est « en démarrage ».
	startingFor = time.Minute
)

// États d'Eyrie annoncés avec le défi.
const (
	eyrieClosed   = "fermee"
	eyrieStarting = "demarrage"
	eyrieOpen     = "ouverte"
	eyrieCut      = "acces-coupe"
)

// Entropie DPAPI de mobile\veille.dpapi (celle d'Eyrie, qui l'écrit).
var wakeEntropy = []byte("eyrie/veille")

// Le nom du mutex d'instance d'Eyrie commence ainsi (crates/eyrie/src/main.rs).
const instancePrefix = `Local\Eyrie-instance:`

// wakeConfig : mobile\veille.dpapi.
type wakeConfig struct {
	V      int  `json:"v"`
	Active bool `json:"actif"`
	// Clé du veilleur (32 octets, hexadécimal) : les secrets des appareils en sont tirés.
	Key string `json:"cle"`
	// Le mutex d'instance d'Eyrie, tenu tant qu'elle est ouverte.
	Instance string `json:"instance"`
	// Eyrie démarrée à distance s'ouvre réduite.
	Minimized bool `json:"reduite"`
}

// openShared ouvre `path` en lecture sans gêner qui le remplace au même moment. Eyrie enregistre appareils.json et
// veille.dpapi dans un fichier temporaire qu'elle renomme par-dessus : os.Open, qui ne partage pas la suppression,
// ferait échouer ce renommage (un appareil retiré non enregistré, un accès coupé resté ouvert).
func openShared(path string) (*os.File, error) {
	p, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return nil, err
	}
	h, err := windows.CreateFile(p, windows.GENERIC_READ,
		windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE, nil, windows.OPEN_EXISTING,
		windows.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		return nil, &os.PathError{Op: "open", Path: path, Err: err}
	}
	return os.NewFile(uintptr(h), path), nil
}

// readShared lit tout `path` (voir openShared).
func readShared(path string) ([]byte, error) {
	f, err := openShared(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	return io.ReadAll(f)
}

// readWakeConfig relit mobile\veille.dpapi ; rend aussi la clé du veilleur.
func readWakeConfig(mobile string) (wakeConfig, []byte, error) {
	var cfg wakeConfig
	sealed, err := readShared(filepath.Join(mobile, "veille.dpapi"))
	if err != nil {
		return cfg, nil, fmt.Errorf("réglage du veilleur : %w", err)
	}
	raw, err := unseal(sealed, wakeEntropy)
	if err != nil {
		return cfg, nil, fmt.Errorf("réglage du veilleur indéchiffrable : %w", err)
	}
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return cfg, nil, fmt.Errorf("réglage du veilleur illisible : %w", err)
	}
	if cfg.V != 1 {
		return cfg, nil, fmt.Errorf("réglage du veilleur : version %d inconnue", cfg.V)
	}
	key, err := hex.DecodeString(cfg.Key)
	if err != nil || len(key) != 32 {
		return cfg, nil, errors.New("réglage du veilleur : clé invalide")
	}
	if !strings.HasPrefix(cfg.Instance, instancePrefix) {
		return cfg, nil, fmt.Errorf("réglage du veilleur : instance inattendue %q", cfg.Instance)
	}
	return cfg, key, nil
}

// pairedDevice : ce que le veilleur lit d'un appareil de mobile\appareils.json (crates/eyrie/src/mobile/store.rs).
type pairedDevice struct {
	ID    string `json:"id"`
	Name  string `json:"name"`
	Key   string `json:"key"`
	Login string `json:"login"`
	// Ses droits (Rights de crates/suite-mobile/src/msg.rs), tels qu'écrits : voir mayAct.
	Rights json.RawMessage `json:"rights"`
}

// mayAct : l'appareil a-t-il le droit « Agir » ? Démarrer Eyrie en est un usage. Sans droits écrits (appairé avant
// eux), oui : Eyrie les lui accorde tous (#[serde(default)]). Des droits sans « act » booléen, qu'Eyrie ne lirait
// pas : non.
func (d pairedDevice) mayAct() bool {
	if d.Rights == nil {
		return true
	}
	var r struct {
		Act *bool `json:"act"`
	}
	if json.Unmarshal(d.Rights, &r) != nil || r.Act == nil {
		return false
	}
	return *r.Act
}

func readDevices(mobile string) ([]pairedDevice, error) {
	raw, err := readShared(filepath.Join(mobile, "appareils.json"))
	if err != nil {
		return nil, err
	}
	var devices []pairedDevice
	if err := json.Unmarshal(raw, &devices); err != nil {
		return nil, fmt.Errorf("appareils.json illisible : %w", err)
	}
	return devices, nil
}

func wakeMAC(key []byte) hash.Hash {
	m, err := blake2s.New256(key)
	if err != nil {
		panic(err) // clé de 32 octets au plus
	}
	return m
}

// field : un champ de longueur variable, précédé de sa longueur (gros-boutiste) : aucune concaténation ambiguë.
func field(m hash.Hash, b []byte) {
	m.Write(binary.BigEndian.AppendUint32(nil, uint32(len(b))))
	m.Write(b)
}

// deviceSecret : le secret de démarrage d'un appareil, MAC de sa clé publique par la clé du veilleur
// (veille::device_secret).
func deviceSecret(pcKey, devicePublic []byte) []byte {
	m := wakeMAC(pcKey)
	m.Write([]byte("eyrie-veille/appareil"))
	m.Write(devicePublic)
	return m.Sum(nil)
}

// wakeProof : la preuve que l'appareil `device` connaît son secret, liée au défi et à la demande (veille::proof).
func wakeProof(secret, challenge []byte, device, request string) []byte {
	m := wakeMAC(secret)
	m.Write([]byte("eyrie-veille/1"))
	m.Write(challenge)
	field(m, []byte(device))
	field(m, []byte(request))
	return m.Sum(nil)
}

// instanceOpen : Eyrie tient-elle son mutex d'instance ? Un mutex qui existe mais ne s'ouvre pas compte.
func instanceOpen(name string) bool {
	p, err := windows.UTF16PtrFromString(name)
	if err != nil {
		return false
	}
	h, err := windows.OpenMutex(windows.SYNCHRONIZE, false, p)
	if err == nil {
		windows.CloseHandle(h)
		return true
	}
	return !errors.Is(err, windows.ERROR_FILE_NOT_FOUND)
}

type wakeHello struct {
	V         int    `json:"v"`
	Challenge string `json:"defi"`
	Eyrie     string `json:"eyrie"`
}

type wakeRequest struct {
	Device  string `json:"appareil"`
	Request string `json:"demande"`
	Proof   string `json:"preuve"`
}

type wakeReply struct {
	OK     bool   `json:"ok"`
	State  string `json:"etat,omitempty"`
	Reason string `json:"raison,omitempty"`
}

// strikes : les preuves fausses d'un appareil du tailnet, et jusqu'à quand il est bloqué.
type strikes struct {
	count int
	until time.Time
}

// waker : le démarrage d'Eyrie à la demande d'un téléphone.
type waker struct {
	// Le dossier du pont (appareils.json, veille.dpapi, journal.jsonl).
	mobile string
	// eyrie.exe, à côté du veilleur.
	exe    string
	who    whois
	isOpen func(instance string) bool
	check  func(exe string) error
	launch func(exe string, args, env []string) (child, error)
	now    func() time.Time
	logf   func(string, ...any)

	conns atomic.Int32
	mu    sync.Mutex
	// Par adresse sur le tailnet (un appareil).
	wrong  map[string]*strikes
	starts []time.Time
	// L'Eyrie lancée, suivie jusqu'à ce qu'elle soit ouverte, sortie, ou lancée depuis plus de startingFor.
	started  child
	launched time.Time
}

func newWaker(mobile, exe string, who whois, logf func(string, ...any)) *waker {
	return &waker{
		mobile: mobile,
		exe:    exe,
		who:    who,
		isOpen: instanceOpen,
		check:  exePlaceCheck,
		launch: launchEyrie,
		now:    time.Now,
		logf:   logf,
		wrong:  map[string]*strikes{},
	}
}

func (w *waker) serve(ctx context.Context, ln net.Listener) {
	go func() {
		<-ctx.Done()
		ln.Close()
	}()
	for {
		c, err := ln.Accept()
		if err != nil {
			if ctx.Err() != nil {
				return
			}
			w.logf("veille : %v", err)
			time.Sleep(200 * time.Millisecond)
			continue
		}
		if w.conns.Add(1) > maxWakeConns {
			w.conns.Add(-1)
			c.Close()
			continue
		}
		go func() {
			defer w.conns.Add(-1)
			w.handle(ctx, c)
		}()
	}
}

func (w *waker) handle(ctx context.Context, c net.Conn) {
	defer c.Close()
	c.SetDeadline(time.Now().Add(wakeDeadline))
	addr := c.RemoteAddr().String()
	wctx, cancel := context.WithTimeout(ctx, wakeDeadline)
	id, err := w.who(wctx, addr)
	cancel()
	if err != nil || id.Login == "" {
		return
	}
	devices, err := readDevices(w.mobile)
	if err != nil {
		w.logf("veille : %v", err)
		return
	}
	known := false
	for _, d := range devices {
		known = known || (d.Login != "" && d.Login == id.Login)
	}
	if !known {
		return
	}
	cfg, key, err := readWakeConfig(w.mobile)
	if err != nil {
		w.logf("veille : %v", err)
		return
	}
	challenge := make([]byte, 32)
	if _, err := rand.Read(challenge); err != nil {
		return
	}
	if writeJSON(c, wakeHello{V: 1, Challenge: hex.EncodeToString(challenge), Eyrie: w.state(cfg)}) != nil {
		return
	}
	line, err := bufio.NewReaderSize(c, maxWakeLine).ReadSlice('\n')
	if err != nil {
		return
	}
	var req wakeRequest
	if json.Unmarshal(line, &req) != nil {
		return
	}
	peer, _, err := net.SplitHostPort(addr)
	if err != nil {
		peer = addr
	}
	if reason := w.decide(peer, id.Login, devices, cfg, key, challenge, req); reason != "" {
		_ = writeJSON(c, wakeReply{Reason: reason})
	} else {
		_ = writeJSON(c, wakeReply{OK: true, State: eyrieStarting})
	}
}

func writeJSON(c net.Conn, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	_, err = c.Write(append(b, '\n'))
	return err
}

func (w *waker) state(cfg wakeConfig) string {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.stateLocked(cfg)
}

func (w *waker) stateLocked(cfg wakeConfig) string {
	if !cfg.Active {
		return eyrieCut
	}
	if w.isOpen(cfg.Instance) {
		w.forget()
		return eyrieOpen
	}
	// Ouverte puis refermée, ou sortie aussitôt lancée (plantage), elle n'est plus en démarrage : le téléphone peut
	// la redémarrer (dans les limites de startWait).
	if w.started != nil && w.now().Sub(w.launched) < startingFor && w.started.running() {
		return eyrieStarting
	}
	w.forget()
	return eyrieClosed
}

// forget : le veilleur ne suit plus l'Eyrie qu'il a lancée.
func (w *waker) forget() {
	if w.started != nil {
		w.started.release()
		w.started = nil
	}
}

// decide vérifie une demande et démarre Eyrie s'il y a lieu ; rend "" quand Eyrie démarre (ou démarrait déjà), sinon
// la raison du refus, pour le téléphone.
func (w *waker) decide(peer, login string, devices []pairedDevice, cfg wakeConfig, key, challenge []byte, req wakeRequest) string {
	w.mu.Lock()
	defer w.mu.Unlock()
	now := w.now()
	if s := w.wrong[peer]; s != nil && now.Before(s.until) {
		return fmt.Sprintf("trop de preuves fausses : réessayez dans %s", waitText(s.until.Sub(now)))
	}
	var dev *pairedDevice
	for i := range devices {
		if devices[i].ID == req.Device && devices[i].Login == login {
			dev = &devices[i]
		}
	}
	if dev == nil || !proofHolds(dev.Key, key, challenge, req) {
		s := w.wrong[peer]
		if s == nil || !s.until.IsZero() {
			s = &strikes{}
			w.wrong[peer] = s
		}
		s.count++
		name := ""
		if dev != nil {
			name = dev.Name
		}
		text := fmt.Sprintf("Démarrage refusé : preuve fausse (compte %s, appareil « %s »)", login, req.Device)
		if s.count >= maxWrongProofs {
			s.until = now.Add(blockedFor)
			text += fmt.Sprintf(" ; %d preuves fausses, cet appareil est bloqué %s", maxWrongProofs, waitText(blockedFor))
		}
		w.journal(name, text, now)
		return "preuve refusée"
	}
	delete(w.wrong, peer)
	if req.Request != wakeStart {
		return "demande inconnue : " + req.Request
	}
	refuse := func(reason string) string {
		w.journal(dev.Name, "Démarrage refusé : "+reason, now)
		return reason
	}
	// Eyrie ne donne son secret qu'à un appareil qui a « Agir » ; celui-ci l'a reçu avant que le droit lui soit retiré.
	if !dev.mayAct() {
		return refuse(actRefused)
	}
	switch w.stateLocked(cfg) {
	case eyrieCut:
		return refuse("accès depuis le téléphone coupé sur le PC")
	case eyrieOpen:
		return "Eyrie est déjà ouverte"
	case eyrieStarting:
		return ""
	}
	if wait := w.startWait(now); wait > 0 {
		return refuse(fmt.Sprintf("trop de démarrages : réessayez dans %s", waitText(wait)))
	}
	if err := w.check(w.exe); err != nil {
		w.journal(dev.Name, "Démarrage refusé : "+err.Error(), now)
		return placeReason(err)
	}
	var args []string
	if cfg.Minimized {
		args = []string{"--reduite"}
	}
	started, err := w.launch(w.exe, args, []string{"EYRIE_DEMARRE_PAR=" + envText(dev.Name)})
	if err != nil {
		return refuse(err.Error())
	}
	w.starts = append(w.starts, now)
	w.started, w.launched = started, now
	text := "Eyrie démarrée depuis le téléphone"
	if cfg.Minimized {
		text += " (réduite)"
	}
	w.journal(dev.Name, text, now)
	return ""
}

// placeReason : ce que le téléphone apprend d'un emplacement refusé (exePlaceCheck) ; le détail est au journal du PC.
func placeReason(err error) string {
	var place *unsafePlace
	switch {
	case errors.As(err, &place):
		return "Eyrie n'est pas à l'abri sur le PC (modifiable par un autre compte) : voir le journal du PC"
	case errors.Is(err, fs.ErrNotExist):
		return "eyrie.exe introuvable à côté du veilleur sur le PC : voir le journal du PC"
	}
	return "emplacement d'Eyrie illisible sur le PC : voir le journal du PC"
}

// proofHolds : la preuve d'une demande vaut-elle pour l'appareil de clé publique `public` (hexadécimal) ? En temps
// constant.
func proofHolds(public string, pcKey, challenge []byte, req wakeRequest) bool {
	pub, err := hex.DecodeString(public)
	if err != nil || len(pub) != 32 {
		return false
	}
	candidate, err := hex.DecodeString(req.Proof)
	if err != nil {
		return false
	}
	want := wakeProof(deviceSecret(pcKey, pub), challenge, req.Device, req.Request)
	return hmac.Equal(want, candidate)
}

// startWait : le temps à attendre avant le prochain démarrage permis (0 : tout de suite).
func (w *waker) startWait(now time.Time) time.Duration {
	kept := w.starts[:0]
	for _, t := range w.starts {
		if now.Sub(t) < time.Hour {
			kept = append(kept, t)
		}
	}
	w.starts = kept
	var wait time.Duration
	if n := len(kept); n > 0 {
		wait = max(wait, startEvery-now.Sub(kept[n-1]))
	}
	if len(kept) >= startsPerHour {
		wait = max(wait, time.Hour-now.Sub(kept[len(kept)-startsPerHour]))
	}
	return wait
}

// waitText : une attente, pour un humain (« 30 s », « 15 min »).
func waitText(d time.Duration) string {
	if d < time.Minute {
		return fmt.Sprintf("%d s", int((d+time.Second-1)/time.Second))
	}
	return fmt.Sprintf("%d min", int((d+time.Minute-1)/time.Minute))
}

// envText : le nom d'un appareil, sans caractère de contrôle, 64 caractères au plus (EYRIE_DEMARRE_PAR).
func envText(s string) string {
	var b strings.Builder
	n := 0
	for _, r := range s {
		if unicode.IsControl(r) {
			continue
		}
		if n == 64 {
			break
		}
		b.WriteRune(r)
		n++
	}
	return b.String()
}

// Le journal du pont (mobile\journal.jsonl) passe à journal.1.jsonl au-delà d'un mégaoctet, comme chez Eyrie.
const journalMax = 1 << 20

// journalLine : une ligne du journal du pont (JournalLine de crates/eyrie/src/mobile/store.rs).
type journalLine struct {
	At     int64  `json:"at"`
	Device string `json:"device"`
	Kind   string `json:"kind"`
	Text   string `json:"text"`
}

// journal ajoute une ligne « veille » au journal du pont, que la page Téléphones d'Eyrie montre.
func (w *waker) journal(device, text string, at time.Time) {
	if err := writeJournal(w.mobile, device, text, at); err != nil {
		w.logf("veille : journal : %v", err)
	}
}

// writeJournal ajoute une ligne « veille » au journal du pont de `mobile`.
func writeJournal(mobile, device, text string, at time.Time) error {
	path := filepath.Join(mobile, "journal.jsonl")
	if st, err := os.Stat(path); err == nil && st.Size() > journalMax {
		_ = os.Rename(path, filepath.Join(mobile, "journal.1.jsonl"))
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetEscapeHTML(false)
	return enc.Encode(journalLine{At: at.Unix(), Device: device, Kind: "veille", Text: text})
}
