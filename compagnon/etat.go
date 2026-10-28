//go:build windows

package main

// État du nœud (sa clé privée comprise) chiffré par DPAPI : seul le compte Windows de l'utilisateur, sur
// ce poste, peut le relire. Une copie du dossier (sauvegarde, synchronisation, autre compte qui y
// accéderait) ne permet plus de se faire passer pour ce nœud sur le tailnet. Remplace le fichier en
// clair `tailscaled.state` de tsnet, repris puis effacé au premier lancement.

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"
	"unsafe"

	"golang.org/x/sys/windows"
	"tailscale.com/ipn"
)

// Nom du fichier chiffré, et de l'ancien fichier en clair de tsnet.
const (
	sealedFile = "noeud.dpapi"
	plainFile  = "tailscaled.state"
)

// Entropie propre au compagnon : un autre programme du même compte qui déchiffre ses propres données
// DPAPI ne déchiffre pas celles-ci par mégarde (ce n'est pas un secret).
// La même entropie que le compagnon de Fawkes, dont Eyrie a repris le nœud : son état reste lisible.
var entropy = []byte("fawkes-tailnet/etat-du-noeud")

type sealedStore struct {
	path  string
	mu    sync.Mutex
	state map[ipn.StateKey][]byte
}

// openStateStore ouvre l'état chiffré du dossier `dir`, en reprenant l'ancien état en clair s'il est seul.
func openStateStore(dir string) (ipn.StateStore, error) {
	s := &sealedStore{path: filepath.Join(dir, sealedFile), state: map[ipn.StateKey][]byte{}}
	plain := filepath.Join(dir, plainFile)
	sealed, err := os.ReadFile(s.path)
	switch {
	case err == nil:
		raw, err := unprotect(sealed)
		if err != nil {
			return nil, fmt.Errorf("état du nœud indéchiffrable (autre compte ou autre poste ?) : %w", err)
		}
		if err := json.Unmarshal(raw, &s.state); err != nil {
			return nil, fmt.Errorf("état du nœud illisible : %w", err)
		}
	case errors.Is(err, fs.ErrNotExist):
		if raw, err := os.ReadFile(plain); err == nil {
			if err := json.Unmarshal(raw, &s.state); err != nil {
				return nil, fmt.Errorf("ancien état du nœud illisible : %w", err)
			}
			if err := s.save(); err != nil {
				return nil, err
			}
		}
	default:
		return nil, fmt.Errorf("état du nœud : %w", err)
	}
	// Une fois l'état chiffré écrit, plus de copie en clair.
	if _, err := os.Stat(s.path); err == nil {
		_ = os.Remove(plain)
	}
	return s, nil
}

func (s *sealedStore) ReadState(id ipn.StateKey) ([]byte, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	bs, ok := s.state[id]
	if !ok {
		return nil, ipn.ErrStateNotExist
	}
	return bytes.Clone(bs), nil
}

func (s *sealedStore) WriteState(id ipn.StateKey, bs []byte) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if bs == nil {
		delete(s.state, id)
	} else {
		s.state[id] = bytes.Clone(bs)
	}
	return s.save()
}

// save chiffre tout l'état et l'écrit d'un coup (fichier temporaire puis renommage).
func (s *sealedStore) save() error {
	raw, err := json.Marshal(s.state)
	if err != nil {
		return err
	}
	sealed, err := protect(raw)
	if err != nil {
		return fmt.Errorf("chiffrement de l'état du nœud : %w", err)
	}
	tmp := s.path + ".tmp"
	if err := os.WriteFile(tmp, sealed, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, s.path)
}

func blob(b []byte) *windows.DataBlob {
	if len(b) == 0 {
		return &windows.DataBlob{}
	}
	return &windows.DataBlob{Size: uint32(len(b)), Data: &b[0]}
}

// taken copie le résultat d'un appel DPAPI puis libère sa mémoire.
func taken(out windows.DataBlob) []byte {
	defer windows.LocalFree(windows.Handle(unsafe.Pointer(out.Data)))
	return bytes.Clone(unsafe.Slice(out.Data, out.Size))
}

func protect(b []byte) ([]byte, error) {
	return seal(b, entropy, "Eyrie : état du nœud Tailscale")
}

func unprotect(b []byte) ([]byte, error) {
	return unseal(b, entropy)
}

// seal chiffre `b` pour le compte de l'utilisateur sur ce poste (DPAPI), avec l'entropie `ent`.
func seal(b, ent []byte, description string) ([]byte, error) {
	var out windows.DataBlob
	desc, _ := windows.UTF16PtrFromString(description)
	if err := windows.CryptProtectData(blob(b), desc, blob(ent), 0, nil, windows.CRYPTPROTECT_UI_FORBIDDEN, &out); err != nil {
		return nil, err
	}
	return taken(out), nil
}

// unseal déchiffre ce que seal (ou Eyrie, par DPAPI) a chiffré avec l'entropie `ent`.
func unseal(b, ent []byte) ([]byte, error) {
	var out windows.DataBlob
	if err := windows.CryptUnprotectData(blob(b), nil, blob(ent), 0, nil, windows.CRYPTPROTECT_UI_FORBIDDEN, &out); err != nil {
		return nil, err
	}
	return taken(out), nil
}
