package noeud

import (
	"net/netip"
	"testing"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/types/key"
)

// status : l'état d'un nœud en ligne dans le tailnet exemple.ts.net, avec ces appareils.
func status(peers ...*ipnstate.PeerStatus) *ipnstate.Status {
	st := &ipnstate.Status{
		BackendState:   "Running",
		Self:           &ipnstate.PeerStatus{Online: true},
		CurrentTailnet: &ipnstate.TailnetStatus{MagicDNSSuffix: "exemple.ts.net"},
		Peer:           map[key.NodePublic]*ipnstate.PeerStatus{},
	}
	for _, p := range peers {
		st.Peer[key.NewNode().Public()] = p
	}
	return st
}

func find(t *testing.T, st *ipnstate.Status, addr string, now time.Time) Pair {
	t.Helper()
	got, ok := ChercherPair(st, addr, now)
	if !ok {
		t.Fatalf("%s : état jugé périmé", addr)
	}
	return got
}

// Le PC se retrouve par son adresse du tailnet ou son nom (complet ou court, avec ou sans port) : en ligne, ou vu
// pour la dernière fois à telle heure.
func TestThePcIsFoundByAddressOrName(t *testing.T) {
	now := time.Unix(1_800_000_000, 0)
	seen := time.Unix(1_799_990_000, 0)
	pc := &ipnstate.PeerStatus{
		HostName:     "SUITE-PC",
		DNSName:      "suite-pc.exemple.ts.net.",
		TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.1.2.3"), netip.MustParseAddr("fd7a:115c:a1e0::1")},
		LastSeen:     seen,
	}
	other := &ipnstate.PeerStatus{
		HostName:     "pixel",
		DNSName:      "pixel.exemple.ts.net.",
		TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.9.9.9")},
		Online:       true,
	}
	st := status(pc, other)
	for _, addr := range []string{
		"100.1.2.3:4747", "100.1.2.3", "[fd7a:115c:a1e0::1]:4747", "suite-pc:4747", "Suite-PC.exemple.ts.net.:4747",
		"suite-pc.exemple.ts.net",
	} {
		if got := find(t, st, addr, now); got != (Pair{Connu: true, EnLigne: false, Vu: seen.Unix()}) {
			t.Errorf("%s : %+v", addr, got)
		}
	}
	pc.Online = true
	if got := find(t, st, "suite-pc:4747", now); got != (Pair{Connu: true, EnLigne: true, Vu: now.Unix()}) {
		t.Errorf("en ligne : %+v", got)
	}
	for _, addr := range []string{"100.1.2.4:4747", "autre-pc:4747", "suite:4747", ""} {
		if got := find(t, st, addr, now); got != (Pair{}) {
			t.Errorf("%q : %+v", addr, got)
		}
	}
}

// Deux appareils du même nom court (un ancien PC resté dans le tailnet) : celui qui est en ligne.
func TestTheOnlineNamesakeWins(t *testing.T) {
	old := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc-1.exemple.ts.net."}
	cur := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc.exemple.ts.net.", Online: true}
	for range 20 {
		if got := find(t, status(old, cur), "suite-pc", time.Unix(5, 0)); !got.EnLigne {
			t.Fatalf("%+v", got)
		}
	}
}

// Le nom que MagicDNS résout l'emporte sur un homonyme en ligne : un autre appareil du même nom d'hôte (renommé
// suite-pc-1), ou du même nom court dans un autre tailnet (partagé). Sinon un PC éteint passerait pour en ligne.
func TestTheMagicDNSNameBeatsANamesakeOnline(t *testing.T) {
	pc := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc.exemple.ts.net."}
	renamed := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc-1.exemple.ts.net.", Online: true}
	shared := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc.autre.ts.net.", Online: true}
	for range 20 {
		if got := find(t, status(pc, renamed, shared), "suite-pc:4747", time.Unix(5, 0)); !got.Connu || got.EnLigne {
			t.Fatalf("%+v", got)
		}
	}
	// Sans nom MagicDNS qui corresponde, le nom d'hôte suffit.
	if got := find(t, status(renamed), "suite-pc:4747", time.Unix(5, 0)); !got.EnLigne {
		t.Fatalf("nom d'hôte seul : %+v", got)
	}
}

// Un nœud qui n'est pas lui-même en ligne ne dit rien des autres : ce qu'il en sait est périmé.
func TestAStaleNodeSaysNothing(t *testing.T) {
	pc := &ipnstate.PeerStatus{DNSName: "suite-pc.exemple.ts.net.", Online: true}
	for what, change := range map[string]func(*ipnstate.Status){
		"hors ligne":       func(st *ipnstate.Status) { st.Self.Online = false },
		"sans lui-même":    func(st *ipnstate.Status) { st.Self = nil },
		"pas en route":     func(st *ipnstate.Status) { st.BackendState = "NeedsLogin" },
		"en cours d'arrêt": func(st *ipnstate.Status) { st.BackendState = "Stopped" },
	} {
		st := status(pc)
		change(st)
		if got, ok := ChercherPair(st, "suite-pc:4747", time.Unix(5, 0)); ok {
			t.Errorf("%s : %+v", what, got)
		}
	}
}
