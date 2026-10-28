package noeud

import (
	"net/netip"
	"testing"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/types/key"
)

func status(peers ...*ipnstate.PeerStatus) *ipnstate.Status {
	st := &ipnstate.Status{Peer: map[key.NodePublic]*ipnstate.PeerStatus{}}
	for _, p := range peers {
		st.Peer[key.NewNode().Public()] = p
	}
	return st
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
		if got := ChercherPair(st, addr, now); got != (Pair{Connu: true, EnLigne: false, Vu: seen.Unix()}) {
			t.Errorf("%s : %+v", addr, got)
		}
	}
	pc.Online = true
	if got := ChercherPair(st, "suite-pc:4747", now); got != (Pair{Connu: true, EnLigne: true, Vu: now.Unix()}) {
		t.Errorf("en ligne : %+v", got)
	}
	for _, addr := range []string{"100.1.2.4:4747", "autre-pc:4747", "suite:4747", ""} {
		if got := ChercherPair(st, addr, now); got != (Pair{}) {
			t.Errorf("%q : %+v", addr, got)
		}
	}
}

// Deux appareils du même nom court (un ancien PC resté dans le tailnet) : celui qui est en ligne.
func TestTheOnlineNamesakeWins(t *testing.T) {
	old := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc-1.exemple.ts.net."}
	cur := &ipnstate.PeerStatus{HostName: "suite-pc", DNSName: "suite-pc.exemple.ts.net.", Online: true}
	for range 20 {
		if got := ChercherPair(status(old, cur), "suite-pc", time.Unix(5, 0)); !got.EnLigne {
			t.Fatalf("%+v", got)
		}
	}
}
