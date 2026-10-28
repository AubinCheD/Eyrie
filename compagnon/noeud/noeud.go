// Package noeud : ce que partagent le compagnon du PC et le nœud du téléphone (bibliothèque de l'application
// Android, ../telephone) : le suivi de l'état d'un nœud tsnet, et le relais d'une connexion dans les deux sens.
package noeud

import (
	"context"
	"io"
	"strings"
	"sync"

	"tailscale.com/client/local"
	"tailscale.com/ipn"
)

// Suivre suit l'état du nœud et le rapporte : `state` vaut starting, needs_login (avec `url`, le lien de
// connexion à ouvrir dans un navigateur), running (avec `name` et `ip`) ou error (avec `message`). Il demande un
// lien de connexion quand le nœud n'est pas connecté, y compris après une déconnexion. Rend la main quand ctx est
// fini ou le suivi rompu.
func Suivre(ctx context.Context, lc *local.Client, report func(map[string]string)) {
	fail := func(msg string) { report(map[string]string{"state": "error", "message": msg}) }
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
			report(map[string]string{"state": "needs_login", "url": *n.BrowseToURL})
		}
		if n.State == nil {
			continue
		}
		switch *n.State {
		case ipn.Starting:
			report(map[string]string{"state": "starting"})
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
			asked = false
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
			report(map[string]string{"state": "running", "name": strings.TrimSuffix(st.Self.DNSName, "."), "ip": ip})
		}
	}
}

// Relier relaie a et b dans les deux sens ; un sens fermé ferme l'autre (l'un des deux bouts a raccroché). Rend
// la main quand tout est fermé.
func Relier(a, b io.ReadWriteCloser) {
	var wg sync.WaitGroup
	wg.Add(2)
	pipe := func(dst, src io.ReadWriteCloser) {
		defer wg.Done()
		_, _ = io.Copy(dst, src)
		dst.Close()
		src.Close()
	}
	go pipe(a, b)
	go pipe(b, a)
	wg.Wait()
}
