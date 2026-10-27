package main

import (
	"bufio"
	"context"
	"encoding/json"
	"io"
	"net"
	"testing"
	"time"
)

func TestOnlyALoopbackTargetWithATokenIsAccepted(t *testing.T) {
	var r relay
	if err := r.configure(`{"relais":"127.0.0.1:5000","jeton":"abc"}`); err != nil {
		t.Fatal(err)
	}
	if r.current.Load() == nil {
		t.Fatal("consigne ignorée")
	}
	for _, bad := range []string{
		`{"relais":"100.64.0.1:5000","jeton":"abc"}`,
		`{"relais":"exemple.org:80","jeton":"abc"}`,
		`{"relais":"127.0.0.1:5000"}`,
		`pas du json`,
	} {
		if err := r.configure(bad); err == nil {
			t.Errorf("acceptée : %s", bad)
		}
	}
	if err := r.configure(`{"relais":""}`); err != nil || r.current.Load() != nil {
		t.Fatal("une consigne vide coupe le relais")
	}
}

// Une connexion relayée arrive chez Eyrie précédée de l'en-tête (jeton, identité Tailscale), puis les
// octets passent dans les deux sens.
func TestAConnectionIsRelayedWithItsHeader(t *testing.T) {
	eyrie, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer eyrie.Close()
	tailnet, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var r relay
	if err := r.configure(`{"relais":"` + eyrie.Addr().String() + `","jeton":"secret"}`); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	who := func(context.Context, string) (identity, error) {
		return identity{Login: "aubin@exemple.org", Node: "pixel-8", Address: "100.1.2.3:4444"}, nil
	}
	go r.serve(ctx, tailnet, who, t.Logf)

	phone, err := net.Dial("tcp", tailnet.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer phone.Close()
	if _, err := phone.Write([]byte("bonjour")); err != nil {
		t.Fatal(err)
	}
	conn, err := eyrie.Accept()
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	conn.SetDeadline(time.Now().Add(5 * time.Second))
	rd := bufio.NewReader(conn)
	line, err := rd.ReadBytes('\n')
	if err != nil {
		t.Fatal(err)
	}
	var h map[string]string
	if err := json.Unmarshal(line, &h); err != nil {
		t.Fatal(err)
	}
	if h["jeton"] != "secret" || h["login"] != "aubin@exemple.org" || h["noeud"] != "pixel-8" {
		t.Fatalf("en-tête : %v", h)
	}
	got := make([]byte, 7)
	if _, err := io.ReadFull(rd, got); err != nil || string(got) != "bonjour" {
		t.Fatalf("relayé : %q %v", got, err)
	}
	if _, err := conn.Write([]byte("salut")); err != nil {
		t.Fatal(err)
	}
	phone.SetDeadline(time.Now().Add(5 * time.Second))
	back := make([]byte, 5)
	if _, err := io.ReadFull(phone, back); err != nil || string(back) != "salut" {
		t.Fatalf("retour : %q %v", back, err)
	}
}

// Relais coupé (accès depuis le téléphone coupé dans Eyrie) : la connexion est fermée sans rien relayer.
func TestNothingIsRelayedWhenAccessIsOff(t *testing.T) {
	tailnet, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var r relay
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go r.serve(ctx, tailnet, func(context.Context, string) (identity, error) { return identity{}, nil }, t.Logf)
	phone, err := net.Dial("tcp", tailnet.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer phone.Close()
	phone.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := phone.Read(make([]byte, 1)); err == nil {
		t.Fatal("la connexion aurait dû être fermée")
	}
}
