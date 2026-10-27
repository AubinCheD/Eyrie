package main

// Le relais du téléphone : chaque connexion reçue sur le tailnet est relayée vers le pont mobile d'Eyrie,
// sur la boucle locale seulement, précédée d'une ligne d'en-tête : le jeton qu'Eyrie a donné (une
// connexion locale sans lui est refusée par Eyrie) et l'identité que Tailscale donne de l'appareil
// (WhoIs). Le contenu (chiffré de bout en bout par Eyrie et le téléphone) est relayé tel quel.

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"sync"
	"sync/atomic"
	"time"
)

// Connexions relayées en même temps, au plus.
const maxConns = 16

type identity struct {
	Login   string
	Node    string
	Address string
}

// La consigne d'Eyrie : où relayer, et avec quel jeton ; vide : plus rien n'est relayé.
type target struct {
	Addr  string `json:"relais"`
	Token string `json:"jeton"`
}

type relay struct {
	current atomic.Pointer[target]
	active  atomic.Int32
}

// configure lit une consigne d'Eyrie. Seule une adresse de la boucle locale est acceptée.
func (r *relay) configure(line string) error {
	var t target
	if err := json.Unmarshal([]byte(line), &t); err != nil {
		return err
	}
	if t.Addr == "" {
		r.current.Store(nil)
		return nil
	}
	host, _, err := net.SplitHostPort(t.Addr)
	if err != nil {
		return err
	}
	if ip := net.ParseIP(host); ip == nil || !ip.IsLoopback() {
		return errors.New("adresse hors de la boucle locale : " + t.Addr)
	}
	if t.Token == "" {
		return errors.New("jeton manquant")
	}
	r.current.Store(&t)
	return nil
}

type whois func(ctx context.Context, addr string) (identity, error)

func (r *relay) serve(ctx context.Context, ln net.Listener, who whois, logf func(string, ...any)) {
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
			logf("relais : %v", err)
			time.Sleep(200 * time.Millisecond)
			continue
		}
		if r.active.Add(1) > maxConns {
			r.active.Add(-1)
			c.Close()
			continue
		}
		go func() {
			defer r.active.Add(-1)
			r.handle(ctx, c, who, logf)
		}()
	}
}

func (r *relay) handle(ctx context.Context, c net.Conn, who whois, logf func(string, ...any)) {
	defer c.Close()
	t := r.current.Load()
	if t == nil {
		// Accès depuis le téléphone coupé dans Eyrie.
		return
	}
	wctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	id, err := who(wctx, c.RemoteAddr().String())
	cancel()
	if err != nil {
		logf("relais : appareil inconnu de Tailscale (%s) : %v", c.RemoteAddr(), err)
		return
	}
	local, err := net.DialTimeout("tcp", t.Addr, 3*time.Second)
	if err != nil {
		logf("relais : Eyrie injoignable : %v", err)
		return
	}
	defer local.Close()
	header, _ := json.Marshal(map[string]string{
		"jeton":   t.Token,
		"login":   id.Login,
		"noeud":   id.Node,
		"adresse": id.Address,
	})
	if _, err := local.Write(append(header, '\n')); err != nil {
		return
	}
	var wg sync.WaitGroup
	wg.Add(2)
	pipe := func(dst, src net.Conn) {
		defer wg.Done()
		_, _ = io.Copy(dst, src)
		// Un sens fermé ferme l'autre : le téléphone ou Eyrie a raccroché.
		dst.Close()
		src.Close()
	}
	go pipe(local, c)
	go pipe(c, local)
	wg.Wait()
}
