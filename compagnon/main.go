// Compagnon d'Eyrie : rejoint le tailnet personnel comme un appareil de plus (tsnet), avec sa propre pile
// réseau en espace utilisateur, sans carte réseau virtuelle ni route système. Il ne touche donc pas au
// client Tailscale installé sur le PC, qui peut rester sur un autre tailnet.
//
// Il annonce son état à Eyrie par lignes JSON sur sa sortie standard, et s'arrête quand Eyrie ferme son
// entrée standard. Il relaie les connexions du téléphone (port --port du tailnet) vers le pont mobile
// d'Eyrie, sur la boucle locale : Eyrie lui en donne l'adresse et le jeton par son entrée standard
// (`relais.go`). Repris du compagnon de Fawkes (même magasin d'état chiffré, `etat.go`).
//
// Avec --veille, c'est le veilleur : lancé à l'ouverture de session, il garde le nœud quand Eyrie est fermée, et
// Eyrie s'y rattache par un tube nommé au lieu de lancer un compagnon (`tube.go`).
//
// Un seul compagnon par nœud (`verrou.go`). Codes de sortie : 0 arrêt demandé, 1 échec, 2 arguments
// manquants, 3 nœud déjà tenu par un autre compagnon.
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"eyrie-tailnet/noeud"

	"tailscale.com/logtail"
	"tailscale.com/tsnet"
)

// Codes de sortie.
const (
	exitOK     = 0
	exitFailed = 1
	exitUsage  = 2
	exitHeld   = 3
)

// statusLines : où vont les lignes d'état pour Eyrie. La dernière est gardée, pour une Eyrie qui se rattache
// après coup (veilleur, `tube.go`).
type statusLines struct {
	mu   sync.Mutex
	w    io.Writer
	last []byte
}

var status = &statusLines{w: os.Stdout}

func (s *statusLines) emit(fields map[string]string) {
	line, _ := json.Marshal(fields)
	line = append(line, '\n')
	s.mu.Lock()
	defer s.mu.Unlock()
	s.last = line
	if s.w != nil {
		_, _ = s.w.Write(line)
	}
}

// attach envoie désormais les lignes à `w`, en commençant par la dernière.
func (s *statusLines) attach(w io.Writer) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.w = w
	if s.last != nil {
		_, _ = w.Write(s.last)
	}
}

// detach : plus rien n'est envoyé à `w` (s'il était encore le destinataire).
func (s *statusLines) detach(w io.Writer) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.w == w {
		s.w = nil
	}
}

// emit envoie une ligne d'état à Eyrie.
func emit(fields map[string]string) {
	status.emit(fields)
}

func fail(msg string) {
	emit(map[string]string{"state": "error", "message": msg})
}

// control applique une consigne d'Eyrie (une ligne) : une consigne de relais (`relais.go`), ou {"arret":true},
// pour laquelle elle rend vrai.
func control(r *relay, line string) (stop bool, err error) {
	var c struct {
		Stop bool `json:"arret"`
	}
	if json.Unmarshal([]byte(line), &c) == nil && c.Stop {
		return true, nil
	}
	return false, r.configure(line)
}

func main() {
	os.Exit(run(os.Args[1:]))
}

func run(args []string) int {
	fl := flag.NewFlagSet("eyrie-tailnet", flag.ContinueOnError)
	dir := fl.String("dir", "", "dossier d'état du nœud (sa clé comprise)")
	hostname := fl.String("hostname", "eyrie", "nom d'Eyrie dans le tailnet")
	version := fl.String("version", "", "version d'Eyrie")
	port := fl.Int("port", 4747, "port du tailnet où le téléphone se connecte")
	standby := fl.Bool("veille", false, "veilleur : garde le nœud quand Eyrie est fermée, piloté par un tube nommé")
	if err := fl.Parse(args); err != nil {
		return exitUsage
	}
	_ = version
	if *dir == "" {
		fail("dossier d'état manquant (--dir)")
		return exitUsage
	}
	if err := os.MkdirAll(*dir, 0o700); err != nil {
		fail("dossier d'état : " + err.Error())
		return exitFailed
	}
	// Avant tout le reste (le journal du nœud compris, qu'un second compagnon viderait).
	lock, err := lockNode(*dir)
	if err != nil {
		fail(err.Error())
		if errors.Is(err, errNodeHeld) {
			return exitHeld
		}
		return exitFailed
	}
	defer lock.Close()
	// Les journaux de tsnet restent sur le poste : rien n'est envoyé à Tailscale. tsnet construit son
	// journal distant sans consulter TS_NO_LOGS_NO_SUPPORT : seul logtail.Disable() le coupe à coup
	// sûr, pour tout le processus.
	os.Setenv("TS_NO_LOGS_NO_SUPPORT", "true")
	logtail.Disable()
	logFile, err := os.Create(filepath.Join(*dir, "journal.txt"))
	if err != nil {
		logFile = nil
	} else {
		defer logFile.Close()
	}
	logf := func(format string, args ...any) {
		if logFile != nil {
			fmt.Fprintf(logFile, format+"\n", args...)
		}
	}

	emit(map[string]string{"state": "starting"})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	relay := &relay{}
	if *standby {
		// L'entrée standard est ignorée : Eyrie se rattache par le tube, avant même que le nœud soit en route.
		ln, err := listenControl(*dir)
		if err != nil {
			fail("tube de contrôle : " + err.Error())
			return exitFailed
		}
		self, err := os.Executable()
		if err != nil {
			fail("exécutable du veilleur : " + err.Error())
			return exitFailed
		}
		here := filepath.Dir(self)
		allowed := []string{filepath.Join(here, "eyrie.exe"), filepath.Join(here, "eyrie-tailnet.exe")}
		// Les lignes d'état ne vont plus qu'à l'Eyrie rattachée.
		status.detach(os.Stdout)
		t := &tube{relay: relay, check: func(c net.Conn) error { return checkClient(c, allowed) }, stop: cancel, logf: logf}
		go t.serve(ctx, ln)
	} else {
		go func() {
			// Une consigne de relais par ligne ; entrée fermée : Eyrie s'arrête ou ne veut plus du tailnet.
			sc := bufio.NewScanner(os.Stdin)
			for sc.Scan() {
				line := strings.TrimSpace(sc.Text())
				if line == "" {
					continue
				}
				stop, err := control(relay, line)
				if err != nil {
					logf("consigne de relais refusée : %v", err)
				}
				if stop {
					break
				}
			}
			cancel()
		}()
	}

	// La clé du nœud n'est jamais écrite en clair (voir etat.go).
	store, err := openStateStore(*dir)
	if err != nil {
		fail(err.Error())
		return exitFailed
	}
	srv := &tsnet.Server{Dir: *dir, Store: store, Hostname: *hostname, Logf: logf, UserLogf: logf}
	defer srv.Close()
	if err := srv.Start(); err != nil {
		fail("démarrage : " + err.Error())
		return exitFailed
	}
	lc, err := srv.LocalClient()
	if err != nil {
		fail("client local : " + err.Error())
		return exitFailed
	}
	go noeud.Suivre(ctx, lc, emit)

	ln, err := srv.Listen("tcp", fmt.Sprintf(":%d", *port))
	if err != nil {
		fail("écoute sur le tailnet : " + err.Error())
		return exitFailed
	}
	go relay.serve(ctx, ln, func(ctx context.Context, addr string) (identity, error) {
		who, err := lc.WhoIs(ctx, addr)
		if err != nil {
			return identity{}, err
		}
		id := identity{Address: addr}
		if who.UserProfile != nil {
			id.Login = who.UserProfile.LoginName
		}
		if who.Node != nil {
			id.Node = who.Node.ComputedName
		}
		return id, nil
	}, logf)

	<-ctx.Done()
	return exitOK
}
