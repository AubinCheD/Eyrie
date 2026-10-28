package noeud

import (
	"net"
	"net/netip"
	"slices"
	"strings"
	"time"

	"tailscale.com/ipn"
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
// du tailnet, un nom MagicDNS complet ou court, ou à défaut le nom d'hôte d'un appareil). Le nom court est d'abord
// celui que MagicDNS résout (dans le tailnet du nœud), puis le premier mot d'un nom MagicDNS ; un appareil ne vient
// par son seul nom d'hôte que si aucun nom MagicDNS ne correspond. À égalité, celui qui est en ligne.
//
// Rend faux quand le nœud lui-même n'est pas en ligne (pas connecté au serveur de coordination : téléphone hors
// réseau) : ce qu'il sait des autres est alors périmé, et un PC éteint depuis passerait pour en ligne.
func ChercherPair(st *ipnstate.Status, addr string, now time.Time) (Pair, bool) {
	if st.BackendState != ipn.Running.String() || st.Self == nil || !st.Self.Online {
		return Pair{}, false
	}
	host := addr
	if h, _, err := net.SplitHostPort(addr); err == nil {
		host = h
	}
	host = strings.TrimSuffix(strings.ToLower(host), ".")
	suffix := ""
	if st.CurrentTailnet != nil {
		suffix = strings.Trim(strings.ToLower(st.CurrentTailnet.MagicDNSSuffix), ".")
	}
	var found *ipnstate.PeerStatus
	best := 0
	for _, p := range st.Peer {
		if p == nil {
			continue
		}
		rank := matchRank(p, host, suffix)
		if rank > best || (rank == best && rank > 0 && p.Online && !found.Online) {
			found, best = p, rank
		}
	}
	if found == nil {
		return Pair{}, true
	}
	out := Pair{Connu: true, EnLigne: found.Online}
	switch {
	case found.Online:
		out.Vu = now.Unix()
	case !found.LastSeen.IsZero():
		out.Vu = found.LastSeen.Unix()
	}
	return out, true
}

// matchRank : à quel point l'appareil `p` répond à `host` (en minuscules, sans point final) ; 0 : pas du tout. 3 :
// son adresse, son nom MagicDNS complet, ou le nom court que MagicDNS résout vers lui (`host`.`suffix`) ; 2 : le
// premier mot de son nom MagicDNS (un appareil partagé d'un autre tailnet) ; 1 : son seul nom d'hôte.
func matchRank(p *ipnstate.PeerStatus, host, suffix string) int {
	if host == "" {
		return 0
	}
	if ip, err := netip.ParseAddr(host); err == nil {
		if slices.Contains(p.TailscaleIPs, ip) {
			return 3
		}
		return 0
	}
	full := strings.TrimSuffix(strings.ToLower(p.DNSName), ".")
	short, _, _ := strings.Cut(full, ".")
	switch {
	case full != "" && (host == full || (suffix != "" && host+"."+suffix == full)):
		return 3
	case short != "" && host == short:
		return 2
	case strings.EqualFold(host, p.HostName):
		return 1
	}
	return 0
}
