package io.github.aubinched.eyrie;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.speech.RecognitionListener;
import android.speech.RecognitionService;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * La dictée d'Eyrie : la reconnaissance vocale d'Android (SpeechRecognizer), pilotée depuis Rust (android.rs). Le seul
 * code Java de l'application : NativeActivity ne reçoit ni onActivityResult ni les réponses d'un RecognitionListener.
 *
 * Seul le texte reconnu sort d'ici, vers Rust, qui l'ajoute à la saisie : le son ne va jamais au PC. La reconnaissance
 * du téléphone lui-même d'abord (Android 12 et suivants, quand il l'a), sinon son service par défaut (Google, le plus
 * souvent), qui peut envoyer le son sur Internet, comme le micro du clavier.
 *
 * SpeechRecognizer ne s'utilise que dans le fil principal : chaque appel y est posté, et aucune de ses exceptions n'en
 * sort (elle arrêterait l'application). Ce que dit la reconnaissance part aux fonctions natives, enregistrées par Rust
 * (RegisterNatives), avec le numéro de l'écoute : Rust ignore ce qui reste d'une écoute finie ou annulée.
 */
public final class Dictee implements RecognitionListener {
    /** Le micro est ouvert : on peut parler. */
    static native void prete(int ecoute);

    /** Le texte provisoire, en entier depuis le début de l'écoute. */
    static native void partiel(int ecoute, String texte);

    /** Le texte final (vide : rien de reconnu) ; l'écoute est finie. */
    static native void resultat(int ecoute, String texte);

    /** Une erreur (SpeechRecognizer.ERROR_*, -1 : l'écoute n'a pas démarré) ; l'écoute est finie. */
    static native void erreur(int ecoute, int code);

    /** Le niveau du son (dB, de -2 à 10 environ). */
    static native void niveau(int ecoute, float db);

    private static final Handler PRINCIPAL = new Handler(Looper.getMainLooper());

    /** La reconnaissance en cours, et le numéro de son écoute (fil principal seulement). */
    private static SpeechRecognizer reconnaisseur;
    private static int numero;

    private final int ecoute;
    private final SpeechRecognizer mien;
    private boolean finie;

    private Dictee(int ecoute, SpeechRecognizer mien) {
        this.ecoute = ecoute;
        this.mien = mien;
    }

    /** 0 : pas de reconnaissance vocale ; 1 : celle du téléphone, sans Internet ; 2 : son service par défaut. */
    public static int disponible(Context ctx) {
        if (Build.VERSION.SDK_INT >= 31 && SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx)) {
            return 1;
        }
        return SpeechRecognizer.isRecognitionAvailable(ctx) ? 2 : 0;
    }

    /** Le nom du service de reconnaissance par défaut (« Google »…) ; vide s'il ne se lit pas. */
    public static String service(Context ctx) {
        PackageManager pm = ctx.getPackageManager();
        String paquet = null;
        try {
            String nom = Settings.Secure.getString(ctx.getContentResolver(), "voice_recognition_service");
            ComponentName c = nom == null ? null : ComponentName.unflattenFromString(nom);
            paquet = c == null ? null : c.getPackageName();
        } catch (Throwable e) {
            // Réglage illisible pour une application (Android 12 et suivants) : le seul service installé, s'il l'est.
        }
        try {
            if (paquet == null) {
                List<ResolveInfo> l = pm.queryIntentServices(new Intent(RecognitionService.SERVICE_INTERFACE), 0);
                if (l.size() == 1) {
                    paquet = l.get(0).serviceInfo.packageName;
                }
            }
            return paquet == null ? "" : String.valueOf(pm.getApplicationLabel(pm.getApplicationInfo(paquet, 0)));
        } catch (Throwable e) {
            return "";
        }
    }

    /** Écoute (`surAppareil` : la reconnaissance du téléphone) ; celle d'avant, s'il y en a une, est annulée. */
    public static void ecouter(final Context ctx, final int ecoute, final boolean surAppareil) {
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                fermer();
                try {
                    reconnaisseur = surAppareil && Build.VERSION.SDK_INT >= 31
                            ? SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx)
                            : SpeechRecognizer.createSpeechRecognizer(ctx);
                    numero = ecoute;
                    reconnaisseur.setRecognitionListener(new Dictee(ecoute, reconnaisseur));
                    Intent i = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
                    i.putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
                    i.putExtra(RecognizerIntent.EXTRA_LANGUAGE, langue());
                    i.putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true);
                    i.putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1);
                    i.putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, ctx.getPackageName());
                    reconnaisseur.startListening(i);
                } catch (Throwable e) {
                    fermer();
                    erreur(ecoute, -1);
                }
            }
        });
    }

    /**
     * Finit l'écoute : ce qui a été dit est reconnu, puis le texte final arrive (`resultat`). Si la reconnaissance ne
     * sait pas finir (une exception), l'écoute finit en erreur : pas de texte final à attendre.
     */
    public static void arreter() {
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                if (reconnaisseur == null) {
                    return;
                }
                try {
                    reconnaisseur.stopListening();
                } catch (Throwable e) {
                    int finie = numero;
                    fermer();
                    erreur(finie, SpeechRecognizer.ERROR_CLIENT);
                }
            }
        });
    }

    /** Abandonne l'écoute : plus rien n'arrive d'elle. */
    public static void annuler() {
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                fermer();
            }
        });
    }

    /**
     * Ferme la reconnaissance en cours. Ses exceptions s'arrêtent ici (destroy, quand la liaison au service est déjà
     * défaite : « Service not registered » ; cancel, d'une reconnaissance de fabricant) ; détruite même si cancel lève.
     */
    private static void fermer() {
        SpeechRecognizer r = reconnaisseur;
        reconnaisseur = null;
        if (r == null) {
            return;
        }
        try {
            r.cancel();
        } catch (Throwable e) {
            // Rien à faire : elle est détruite ensuite.
        }
        try {
            r.destroy();
        } catch (Throwable e) {
            // Déjà défaite.
        }
    }

    /** La langue du téléphone (« fr-FR »…), le français à défaut. */
    private static String langue() {
        String l = Locale.getDefault().toLanguageTag();
        return l.isEmpty() || l.equals("und") ? "fr-FR" : l;
    }

    private static String premier(Bundle b) {
        ArrayList<String> l = b == null ? null : b.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
        return l == null || l.isEmpty() || l.get(0) == null ? "" : l.get(0);
    }

    /**
     * Fin de l'écoute (résultat ou erreur) : sa reconnaissance est libérée, hors de son propre appel, si une autre
     * écoute ne l'a pas déjà remplacée.
     */
    private void fin() {
        finie = true;
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                if (reconnaisseur == mien) {
                    fermer();
                }
            }
        });
    }

    @Override
    public void onReadyForSpeech(Bundle params) {
        if (!finie) {
            prete(ecoute);
        }
    }

    @Override
    public void onPartialResults(Bundle partialResults) {
        if (!finie) {
            partiel(ecoute, premier(partialResults));
        }
    }

    @Override
    public void onResults(Bundle results) {
        if (!finie) {
            fin();
            resultat(ecoute, premier(results));
        }
    }

    @Override
    public void onError(int error) {
        if (!finie) {
            fin();
            erreur(ecoute, error);
        }
    }

    @Override
    public void onRmsChanged(float rmsdB) {
        if (!finie) {
            niveau(ecoute, rmsdB);
        }
    }

    @Override
    public void onBeginningOfSpeech() {}

    @Override
    public void onBufferReceived(byte[] buffer) {}

    @Override
    public void onEndOfSpeech() {}

    @Override
    public void onEvent(int eventType, Bundle params) {}
}
