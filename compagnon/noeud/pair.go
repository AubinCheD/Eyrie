package noeud

import (
	"net"
	"net/netip"
	"slices"
	"strings"
	"time"

	"tailscale.com/ipn/ipnstate"
)

// Pair : ce que le nœud sait d'un autre appareil du tailnet. Pour le téléphone, le PC : quand Eyrie ne répond pas,
// il dit si le PC est sur le tailnet (Eyrie fermée) ou non (éteint, en veille, hors réseau).
type Pair struct {
	// Un appareil du tailnet a cette adresse.
	Connu bool `json:"connu"`
	// Il est connecté au serveur de coordination de Tailscale.
	EnLigne bool `json:"enligne"`
	// Sa dernière connexion (secondes Unix) : maintenant s'il est en ligne, 0 si on l'ignore.
	Vu int64 `json:"vu"`
}

// ChercherPair cherche dans `st` l'appareil d'adresse `addr` (« hôte » ou « hôte:port », l'hôte étant une adresse
// du tailnet, un nom MagicDNS complet ou court) ; à nom égal, celui qui est en ligne.
func ChercherPair(st *ipnstate.Status, addr string, now time.Time) Pair {
	host := addr
	if h, _, err := net.SplitHostPort(addr); err == nil {
		host = h
	}
	host = strings.TrimSuffix(strings.ToLower(host), ".")
	ip, ipErr := netip.ParseAddr(host)
	var found *ipnstate.PeerStatus
	for _, p := range st.Peer {
		if p == nil {
			continue
		}
		match := false
		if ipErr == nil {
			match = slices.Contains(p.TailscaleIPs, ip)
		} else {
			full := strings.TrimSuffix(strings.ToLower(p.DNSName), ".")
			short, _, _ := strings.Cut(full, ".")
			match = host != "" && (host == full || host == short || strings.EqualFold(host, p.HostName))
		}
		if match && (found == nil || (p.Online && !found.Online)) {
			found = p
		}
	}
	if found == nil {
		return Pair{}
	}
	out := Pair{Connu: true, EnLigne: found.Online}
	switch {
	case found.Online:
		out.Vu = now.Unix()
	case !found.LastSeen.IsZero():
		out.Vu = found.LastSeen.Unix()
	}
	return out
}
