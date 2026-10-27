// Compagnon d'Eyrie : rejoint le tailnet personnel comme un appareil de plus (tsnet), avec sa propre pile
// réseau en espace utilisateur, sans carte réseau virtuelle ni route système. Il ne touche donc pas au
// client Tailscale installé sur le PC, qui peut rester sur un autre tailnet.
//
// Il annonce son état à Eyrie par lignes JSON sur sa sortie standard, et s'arrête quand Eyrie ferme son
// entrée standard. Il relaie les connexions du téléphone (port --port du tailnet) vers le pont mobile
// d'Eyrie, sur la boucle locale : Eyrie lui en donne l'adresse et le jeton par son entrée standard
// (`relais.go`). Repris du compagnon de Fawkes (même magasin d'état chiffré, `etat.go`).
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"tailscale.com/client/local"
	"tailscale.com/ipn"
	"tailscale.com/logtail"
	"tailscale.com/tsnet"
)

var (
	outMu sync.Mutex
	out   = json.NewEncoder(os.Stdout)
)

// emit envoie une ligne d'état à Eyrie.
func emit(fields map[string]string) {
	outMu.Lock()
	defer outMu.Unlock()
	_ = out.Encode(fields)
}

func fail(msg string) {
	emit(map[string]string{"state": "error", "message": msg})
}

func main() {
	dir := flag.String("dir", "", "dossier d'état du nœud (sa clé comprise)")
	hostname := flag.String("hostname", "eyrie", "nom d'Eyrie dans le tailnet")
	version := flag.String("version", "", "version d'Eyrie")
	port := flag.Int("port", 4747, "port du tailnet où le téléphone se connecte")
	flag.Parse()
	_ = version
	if *dir == "" {
		fail("dossier d'état manquant (--dir)")
		os.Exit(2)
	}
	if err := os.MkdirAll(*dir, 0o700); err != nil {
		fail("dossier d'état : " + err.Error())
		os.Exit(1)
	}
	// Les journaux de tsnet restent sur le poste : rien n'est envoyé à Tailscale. tsnet construit son
	// journal distant sans consulter TS_NO_LOGS_NO_SUPPORT : seul logtail.Disable() le coupe à coup
	// sûr, pour tout le processus.
	os.Setenv("TS_NO_LOGS_NO_SUPPORT", "true")
	logtail.Disable()
	logFile, err := os.Create(filepath.Join(*dir, "journal.txt"))
	if err != nil {
		logFile = nil
	}
	logf := func(format string, args ...any) {
		if logFile != nil {
			fmt.Fprintf(logFile, format+"\n", args...)
		}
	}

	emit(map[string]string{"state": "starting"})
	ctx, cancel := context.WithCancel(context.Background())
	relay := &relay{}
	go func() {
		// Une consigne de relais par ligne ; entrée fermée : Eyrie s'arrête ou ne veut plus du tailnet.
		sc := bufio.NewScanner(os.Stdin)
		for sc.Scan() {
			line := strings.TrimSpace(sc.Text())
			if line == "" {
				continue
			}
			if err := relay.configure(line); err != nil {
				logf("consigne de relais refusée : %v", err)
			}
		}
		cancel()
	}()

	// La clé du nœud n'est jamais écrite en clair (voir etat.go).
	store, err := openStateStore(*dir)
	if err != nil {
		fail(err.Error())
		os.Exit(1)
	}
	srv := &tsnet.Server{Dir: *dir, Store: store, Hostname: *hostname, Logf: logf, UserLogf: logf}
	defer srv.Close()
	if err := srv.Start(); err != nil {
		fail("démarrage : " + err.Error())
		os.Exit(1)
	}
	lc, err := srv.LocalClient()
	if err != nil {
		fail("client local : " + err.Error())
		os.Exit(1)
	}
	go watch(ctx, lc)

	ln, err := srv.Listen("tcp", fmt.Sprintf(":%d", *port))
	if err != nil {
		fail("écoute sur le tailnet : " + err.Error())
		os.Exit(1)
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
}

// watch suit l'état du nœud et le transmet à Eyrie ; demande un lien de connexion au besoin.
func watch(ctx context.Context, lc *local.Client) {
	w, err := lc.WatchIPNBus(ctx, ipn.NotifyInitialState|ipn.NotifyNoPrivateKeys)
	if err != nil {
		fail("suivi de l'état : " + err.Error())
		return
	}
	defer w.Close()
	asked := false
	for {
		n, err := w.Next()
		if err != nil {
			if ctx.Err() == nil {
				fail("suivi de l'état : " + err.Error())
			}
			return
		}
		if n.BrowseToURL != nil && *n.BrowseToURL != "" {
			emit(map[string]string{"state": "needs_login", "url": *n.BrowseToURL})
		}
		if n.State == nil {
			continue
		}
		switch *n.State {
		case ipn.NeedsLogin:
			if !asked {
				asked = true
				if err := lc.StartLoginInteractive(ctx); err != nil {
					fail("demande de connexion : " + err.Error())
				}
			}
		case ipn.NeedsMachineAuth:
			fail("appareil en attente d'approbation dans la console de votre tailnet")
		case ipn.Running:
			st, err := lc.StatusWithoutPeers(ctx)
			if err != nil || st.Self == nil {
				fail("état du nœud illisible")
				continue
			}
			ip := ""
			for _, a := range st.TailscaleIPs {
				if a.Is4() {
					ip = a.String()
					break
				}
			}
			emit(map[string]string{"state": "running", "name": strings.TrimSuffix(st.Self.DNSName, "."), "ip": ip})
		}
	}
}
