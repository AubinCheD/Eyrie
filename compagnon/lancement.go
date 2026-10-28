//go:build windows

package main

// Le veilleur lance Eyrie à la demande d'un téléphone (`veille.go`) : seulement eyrie.exe, rangé à côté de lui, après
// le contrôle de son emplacement (`exeplace.go`), comme si l'utilisateur l'avait ouverte depuis le menu Démarrer.

import (
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"unsafe"

	"golang.org/x/sys/windows"
)

// launchEyrie démarre `exe` avec `args`, dans la session de l'utilisateur, détaché du veilleur : l'environnement de
// l'utilisateur relu (pas celui, figé, de l'ouverture de session), avec `env` en plus ; le dossier de
// l'exécutable comme dossier de travail ; aucun handle hérité ; son propre groupe de processus, hors du job du
// veilleur quand ce job le permet.
func launchEyrie(exe string, args, env []string) error {
	tok, err := windows.OpenCurrentProcessToken()
	if err != nil {
		return fmt.Errorf("compte de l'utilisateur : %w", err)
	}
	defer tok.Close()
	base, err := tok.Environ(false)
	if err != nil {
		return fmt.Errorf("environnement de l'utilisateur : %w", err)
	}
	block, err := envBlock(withEnv(base, env))
	if err != nil {
		return err
	}
	app, err := windows.UTF16PtrFromString(exe)
	if err != nil {
		return err
	}
	cmdline, err := windows.UTF16PtrFromString(windows.ComposeCommandLine(append([]string{exe}, args...)))
	if err != nil {
		return err
	}
	dir, err := windows.UTF16PtrFromString(filepath.Dir(exe))
	if err != nil {
		return err
	}
	si := &windows.StartupInfo{}
	si.Cb = uint32(unsafe.Sizeof(*si))
	var pi windows.ProcessInformation
	flags := uint32(windows.CREATE_NEW_PROCESS_GROUP | windows.CREATE_UNICODE_ENVIRONMENT)
	err = windows.CreateProcess(app, cmdline, nil, nil, false, flags|windows.CREATE_BREAKAWAY_FROM_JOB, &block[0], dir, si, &pi)
	if errors.Is(err, windows.ERROR_ACCESS_DENIED) {
		// Le job du veilleur ne laisse pas partir ses processus : Eyrie y reste.
		err = windows.CreateProcess(app, cmdline, nil, nil, false, flags, &block[0], dir, si, &pi)
	}
	if err != nil {
		return fmt.Errorf("lancement de %s : %w", exe, err)
	}
	windows.CloseHandle(pi.Thread)
	windows.CloseHandle(pi.Process)
	return nil
}

// withEnv : `base` où les variables de `extra` (« NOM=valeur ») remplacent celles du même nom.
func withEnv(base, extra []string) []string {
	replaced := map[string]bool{}
	for _, kv := range extra {
		replaced[envName(kv)] = true
	}
	out := make([]string, 0, len(base)+len(extra))
	for _, kv := range base {
		if !replaced[envName(kv)] {
			out = append(out, kv)
		}
	}
	return append(out, extra...)
}

// envName : le nom d'une variable, sans casse comme sous Windows. Les variables cachées de Windows commencent par
// « = » (« =C:=C:\… »).
func envName(kv string) string {
	start := 0
	if strings.HasPrefix(kv, "=") {
		start = 1
	}
	if i := strings.IndexByte(kv[start:], '='); i >= 0 {
		kv = kv[:start+i]
	}
	return strings.ToUpper(kv)
}

// envBlock : le bloc d'environnement de CreateProcess (UTF-16, « NOM=valeur » terminés par un zéro, puis un zéro).
func envBlock(env []string) ([]uint16, error) {
	var block []uint16
	for _, kv := range env {
		w, err := windows.UTF16FromString(kv)
		if err != nil {
			return nil, fmt.Errorf("variable d'environnement invalide : %q", kv)
		}
		block = append(block, w...)
	}
	if len(block) == 0 {
		block = append(block, 0)
	}
	return append(block, 0), nil
}
