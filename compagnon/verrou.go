//go:build windows

package main

// Un seul compagnon par nœud. Deux processus sur le même état (donc la même clé) se disputeraient le nœud sur le
// tailnet (le serveur de contrôle les ferait alterner, les connexions tomberaient) et l'écriture de son état
// (`etat.go` : même fichier temporaire, puis renommage). Le verrou est un mutex nommé de la session, au nom du
// dossier du nœud : Eyrie le teste avant de lancer un compagnon ou de prendre celui de Fawkes en secours, et se
// rattache au veilleur qui le tient (`tube.go`).

import (
	"errors"
	"strings"
	"unicode"

	"golang.org/x/sys/windows"
)

// errNodeHeld : un autre compagnon tient déjà le nœud.
var errNodeHeld = errors.New("nœud déjà tenu par un autre compagnon")

// dirKey nomme un dossier pour un objet du noyau, exactement comme `dir_key` de crates/suite-sdk/src/instance.rs :
// barres obliques, sans barre finale, minuscules de Rust (rustLower).
func dirKey(dir string) string {
	return rustLower(strings.TrimRight(strings.ReplaceAll(dir, `\`, "/"), "/"))
}

// rustLower : `str::to_lowercase` de Rust. strings.ToLower en diffère pour deux lettres : İ (U+0130) donne « i »
// suivi du point suscrit (U+0307), et Σ en fin de mot donne ς. Sinon, pour un profil comme C:\Users\ΝΙΚΟΣ, Eyrie
// chercherait le verrou et le tube du veilleur sous un autre nom, ne les trouverait pas et lancerait un second
// compagnon, refusé. Vérifié sur tout Unicode contre Rust 1.99 (Go 1.27).
func rustLower(s string) string {
	rs := []rune(s)
	var b strings.Builder
	b.Grow(len(s))
	for i, r := range rs {
		switch r {
		case 'İ':
			b.WriteString("i\u0307")
		case 'Σ':
			// Fin de mot (Final_Sigma d'Unicode) : une lettre à casse avant, aucune après, les signes sans casse
			// (accents, apostrophe, point…) passés.
			if casedNext(rs[:i], true) && !casedNext(rs[i+1:], false) {
				b.WriteRune('ς')
			} else {
				b.WriteRune('σ')
			}
		default:
			b.WriteRune(unicode.ToLower(r))
		}
	}
	return b.String()
}

// casedNext : la première lettre de `rs` (la dernière si `backwards`) qui n'est pas ignorée par la casse
// (Case_Ignorable) a-t-elle une casse (Cased) ?
func casedNext(rs []rune, backwards bool) bool {
	for k := range rs {
		r := rs[k]
		if backwards {
			r = rs[len(rs)-1-k]
		}
		if !caseIgnorable(r) {
			return unicode.In(r, unicode.Lu, unicode.Ll, unicode.Lt, unicode.Other_Lowercase, unicode.Other_Uppercase)
		}
	}
	return false
}

// caseIgnorable : la propriété Case_Ignorable d'Unicode (DerivedCoreProperties), absente du paquet unicode : marques,
// formats, lettres et symboles modificatifs, et les signes Word_Break MidLetter, MidNumLet et Single_Quote.
func caseIgnorable(r rune) bool {
	switch r {
	case '\'', '.', ':', 0xb7, 0x387, 0x55f, 0x5f4, 0x2018, 0x2019, 0x2024, 0x2027, 0xfe13, 0xfe52, 0xfe55, 0xff07,
		0xff0e, 0xff1a:
		return true
	}
	return unicode.In(r, unicode.Mn, unicode.Me, unicode.Cf, unicode.Lm, unicode.Sk)
}

// lockName : le nom du verrou du nœud dont l'état est dans `dir`.
func lockName(dir string) string {
	return `Local\EyrieTailnet:` + dirKey(dir)
}

// nodeLock : le verrou tenu, jusqu'à Close (ou la fin du processus).
type nodeLock struct {
	h windows.Handle
}

// lockNode prend le verrou du nœud `dir` ; errNodeHeld s'il est déjà pris, y compris par ce processus.
func lockNode(dir string) (*nodeLock, error) {
	name, err := windows.UTF16PtrFromString(lockName(dir))
	if err != nil {
		return nil, err
	}
	h, err := windows.CreateMutex(nil, false, name)
	if errors.Is(err, windows.ERROR_ALREADY_EXISTS) {
		windows.CloseHandle(h)
		return nil, errNodeHeld
	}
	if err != nil {
		return nil, err
	}
	return &nodeLock{h: h}, nil
}

func (l *nodeLock) Close() {
	if l.h != 0 {
		windows.CloseHandle(l.h)
		l.h = 0
	}
}
