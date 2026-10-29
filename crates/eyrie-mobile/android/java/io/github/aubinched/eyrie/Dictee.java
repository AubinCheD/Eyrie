package io.github.aubinched.eyrie;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.content.res.Resources;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.provider.Settings;
import android.speech.ModelDownloadListener;
import android.speech.RecognitionListener;
import android.speech.RecognitionService;
import android.speech.RecognitionSupport;
import android.speech.RecognitionSupportCallback;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;
import android.util.Log;
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
 *
 * Chaque étape va au journal d'Android sous l'étiquette « EyrieDictee » (adb logcat -s EyrieDictee) : la
 * reconnaissance choisie, les appels, ce qu'elle répond, les exceptions. Jamais le texte entendu : au plus sa longueur.
 */
public final class Dictee implements RecognitionListener {
    /** Le micro est ouvert : on peut parler (Android est prêt, ou entend déjà parler). */
    static native void prete(int ecoute);

    /** Le texte provisoire, en entier depuis le début de l'écoute. */
    static native void partiel(int ecoute, String texte);

    /** Le texte final (vide : rien de reconnu) ; l'écoute est finie. */
    static native void resultat(int ecoute, String texte);

    /** Une erreur (SpeechRecognizer.ERROR_*, -1 : l'écoute n'a pas démarré) ; l'écoute est finie. */
    static native void erreur(int ecoute, int code);

    /** Le niveau du son (dB, de -2 à 10 environ). */
    static native void niveau(int ecoute, float db);

    /** Le téléchargement de la langue (`telecharger`) : 0 fini, 1 prévu par Android, 2 échoué (`code`). */
    static native void modele(int etat, int code);

    /** L'étiquette du journal d'Android. */
    private static final String JOURNAL = "EyrieDictee";

    private static final Handler PRINCIPAL = new Handler(Looper.getMainLooper());

    /** La reconnaissance en cours, et le numéro de son écoute (fil principal seulement). */
    private static SpeechRecognizer reconnaisseur;
    private static int numero;

    private final int ecoute;
    private final SpeechRecognizer mien;
    private boolean finie;
    /** Pour le journal : l'instant de startListening, ce qui est déjà arrivé ; le micro s'est dit ouvert. */
    private long depart;
    private int partiels;
    private int niveaux;
    private boolean ouvert;

    private Dictee(int ecoute, SpeechRecognizer mien) {
        this.ecoute = ecoute;
        this.mien = mien;
    }

    /** Une étape au journal d'Android (Rust y écrit aussi, par android.rs) : jamais ce qui a été dit. */
    public static void journal(String etape) {
        Log.i(JOURNAL, etape);
    }

    /** Une exception arrêtée ici, au journal : sa classe, son message et sa pile. */
    private static void exception(String ou, Throwable e) {
        Log.w(JOURNAL, ou + " : " + e.getClass().getName() + " : " + e.getMessage(), e);
    }

    /** 0 : pas de reconnaissance vocale ; 1 : celle du téléphone, sans Internet ; 2 : son service par défaut. */
    public static int disponible(Context ctx) {
        int sorte;
        if (Build.VERSION.SDK_INT >= 31 && SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx)) {
            sorte = 1;
        } else {
            sorte = SpeechRecognizer.isRecognitionAvailable(ctx) ? 2 : 0;
        }
        journal("Android " + Build.VERSION.SDK_INT + ", reconnaissance "
                + (sorte == 1 ? "du téléphone (" + composantSurAppareil() + ")"
                        : sorte == 2 ? "par le service par défaut" : "absente")
                + ", langue " + langue());
        return sorte;
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

    /** La reconnaissance du téléphone lui-même, choisie par le fabricant (« com.google.android.as/… ») ; vide sinon. */
    private static String composantSurAppareil() {
        try {
            Resources r = Resources.getSystem();
            int id = r.getIdentifier("config_defaultOnDeviceSpeechRecognitionService", "string", "android");
            return id == 0 ? "" : r.getString(id);
        } catch (Throwable e) {
            return "";
        }
    }

    /** Qui fait la reconnaissance du téléphone, pour la question du téléchargement : « Google », sinon vide. */
    public static String auteur() {
        return composantSurAppareil().startsWith("com.google.") ? "Google" : "";
    }

    /** La reconnaissance du téléphone sait télécharger une langue (Android 13 et suivants). */
    public static boolean peutTelecharger(Context ctx) {
        return Build.VERSION.SDK_INT >= 33 && SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx);
    }

    /** Écoute (`surAppareil` : la reconnaissance du téléphone) ; celle d'avant, s'il y en a une, est annulée. */
    public static void ecouter(final Context ctx, final int ecoute, final boolean surAppareil) {
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                fermer();
                boolean appareil = surAppareil && Build.VERSION.SDK_INT >= 31;
                String langue = langue();
                journal("écoute " + ecoute + " : Android " + Build.VERSION.SDK_INT + ", reconnaissance "
                        + (appareil ? "du téléphone" : "par le service par défaut") + ", langue " + langue);
                String etape = "création";
                try {
                    reconnaisseur = appareil
                            ? SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx)
                            : SpeechRecognizer.createSpeechRecognizer(ctx);
                    numero = ecoute;
                    journal("écoute " + ecoute + " : créée");
                    etape = "setRecognitionListener";
                    Dictee d = new Dictee(ecoute, reconnaisseur);
                    reconnaisseur.setRecognitionListener(d);
                    etape = "startListening";
                    d.depart = SystemClock.elapsedRealtime();
                    reconnaisseur.startListening(intention(ctx, langue));
                    journal("écoute " + ecoute + " : startListening appelé");
                } catch (Throwable e) {
                    exception("écoute " + ecoute + ", " + etape, e);
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
                    journal("stopListening : plus d'écoute");
                    return;
                }
                try {
                    journal("écoute " + numero + " : stopListening");
                    reconnaisseur.stopListening();
                } catch (Throwable e) {
                    exception("écoute " + numero + ", stopListening", e);
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
                journal(reconnaisseur == null ? "annulée : plus d'écoute" : "écoute " + numero + " : annulée");
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
        // Le système note ensuite « Connection to speech recognition service lost, but no #startListening has been
        // invoked yet » : la liaison défaite par destroy, quelle que soit la fin de l'écoute.
        journal("écoute " + numero + " : cancel puis destroy");
        try {
            r.cancel();
        } catch (Throwable e) {
            exception("écoute " + numero + ", cancel", e);
        }
        try {
            r.destroy();
        } catch (Throwable e) {
            exception("écoute " + numero + ", destroy", e);
        }
    }

    /** Ce qui est demandé à la reconnaissance : la langue, les textes provisoires, une seule proposition. */
    private static Intent intention(Context ctx, String langue) {
        Intent i = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
        i.putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
        i.putExtra(RecognizerIntent.EXTRA_LANGUAGE, langue);
        i.putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true);
        i.putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1);
        i.putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, ctx.getPackageName());
        return i;
    }

    /**
     * La langue du téléphone (« fr-FR »…), le français à défaut. Sans ses préférences régionales (Android 14 et suivants
     * : « fr-FR-u-mu-celsius », unité de température, premier jour de la semaine), qu'une reconnaissance ne connaît pas.
     */
    public static String langue() {
        Locale l = Locale.getDefault();
        if (l.getLanguage().isEmpty()) {
            return "fr-FR";
        }
        try {
            return new Locale.Builder().setLanguage(l.getLanguage()).setScript(l.getScript())
                    .setRegion(l.getCountry()).build().toLanguageTag();
        } catch (Throwable e) {
            exception("langue " + l, e);
            return "fr-FR";
        }
    }

    /**
     * Télécharge la langue de la dictée pour la reconnaissance du téléphone (Android 13 et suivants : son modèle, Google
     * le plus souvent) ; ce qu'il en devient part à `modele`. Avant, ce que la reconnaissance a déjà va au journal.
     */
    public static void telecharger(final Context ctx) {
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                if (Build.VERSION.SDK_INT < 33) {
                    journal("téléchargement : Android " + Build.VERSION.SDK_INT + " ne sait pas");
                    modele(2, -1);
                    return;
                }
                final String langue = langue();
                journal("téléchargement de " + langue + " demandé (" + composantSurAppareil() + ")");
                final SpeechRecognizer r;
                try {
                    r = SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx);
                } catch (Throwable e) {
                    exception("téléchargement, création", e);
                    modele(2, -1);
                    return;
                }
                final Intent i = intention(ctx, langue);
                // Un appel après l'autre : deux appels avant la liaison au service en feraient deux.
                try {
                    r.checkRecognitionSupport(i, ctx.getMainExecutor(), new RecognitionSupportCallback() {
                        @Override
                        public void onSupportResult(RecognitionSupport s) {
                            journal("langues du téléphone : installées " + s.getInstalledOnDeviceLanguages()
                                    + ", en cours " + s.getPendingOnDeviceLanguages()
                                    + ", téléchargeables " + s.getSupportedOnDeviceLanguages()
                                    + ", en ligne " + s.getOnlineLanguages());
                            declencher(ctx, r, i);
                        }

                        @Override
                        public void onError(int code) {
                            journal("langues du téléphone : erreur " + code + " (" + nom(code) + ")");
                            declencher(ctx, r, i);
                        }
                    });
                } catch (Throwable e) {
                    exception("téléchargement, checkRecognitionSupport", e);
                    declencher(ctx, r, i);
                }
            }
        });
    }

    /** Le téléchargement lui-même ; la reconnaissance qui l'a demandé est détruite à sa fin. */
    private static void declencher(Context ctx, final SpeechRecognizer r, Intent i) {
        try {
            if (Build.VERSION.SDK_INT >= 34) {
                r.triggerModelDownload(i, ctx.getMainExecutor(), new ModelDownloadListener() {
                    private int dit = -1;

                    @Override
                    public void onProgress(int pourcent) {
                        // Au journal de dix en dix.
                        if (pourcent / 10 != dit) {
                            dit = pourcent / 10;
                            journal("téléchargement : " + pourcent + " %");
                        }
                    }

                    @Override
                    public void onSuccess() {
                        journal("téléchargement : fini");
                        detruire(r);
                        modele(0, 0);
                    }

                    @Override
                    public void onScheduled() {
                        journal("téléchargement : prévu plus tard par Android");
                        detruire(r);
                        modele(1, 0);
                    }

                    @Override
                    public void onError(int code) {
                        journal("téléchargement : erreur " + code + " (" + nom(code) + ")");
                        detruire(r);
                        modele(2, code);
                    }
                });
            } else {
                // Android 13 : demandé, sans nouvelles ensuite. La liaison reste le temps qu'il commence.
                r.triggerModelDownload(i);
                journal("téléchargement : demandé (Android 13, sans suivi)");
                modele(2, SpeechRecognizer.ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS);
                PRINCIPAL.postDelayed(new Runnable() {
                    @Override
                    public void run() {
                        detruire(r);
                    }
                }, 120_000);
            }
        } catch (Throwable e) {
            exception("téléchargement, triggerModelDownload", e);
            detruire(r);
            modele(2, -1);
        }
    }

    private static void detruire(SpeechRecognizer r) {
        try {
            r.destroy();
        } catch (Throwable e) {
            exception("téléchargement, destroy", e);
        }
    }

    /** Le nom d'une erreur de SpeechRecognizer, pour le journal. */
    private static String nom(int code) {
        switch (code) {
            case SpeechRecognizer.ERROR_NETWORK_TIMEOUT: return "NETWORK_TIMEOUT";
            case SpeechRecognizer.ERROR_NETWORK: return "NETWORK";
            case SpeechRecognizer.ERROR_AUDIO: return "AUDIO";
            case SpeechRecognizer.ERROR_SERVER: return "SERVER";
            case SpeechRecognizer.ERROR_CLIENT: return "CLIENT";
            case SpeechRecognizer.ERROR_SPEECH_TIMEOUT: return "SPEECH_TIMEOUT";
            case SpeechRecognizer.ERROR_NO_MATCH: return "NO_MATCH";
            case SpeechRecognizer.ERROR_RECOGNIZER_BUSY: return "RECOGNIZER_BUSY";
            case SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS: return "INSUFFICIENT_PERMISSIONS";
            case SpeechRecognizer.ERROR_TOO_MANY_REQUESTS: return "TOO_MANY_REQUESTS";
            case SpeechRecognizer.ERROR_SERVER_DISCONNECTED: return "SERVER_DISCONNECTED";
            case SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED: return "LANGUAGE_NOT_SUPPORTED";
            case SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE: return "LANGUAGE_UNAVAILABLE";
            case SpeechRecognizer.ERROR_CANNOT_CHECK_SUPPORT: return "CANNOT_CHECK_SUPPORT";
            case SpeechRecognizer.ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS: return "CANNOT_LISTEN_TO_DOWNLOAD_EVENTS";
            default: return "?";
        }
    }

    private static ArrayList<String> textes(Bundle b) {
        return b == null ? null : b.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
    }

    private static String premier(Bundle b) {
        ArrayList<String> l = textes(b);
        return l == null || l.isEmpty() || l.get(0) == null ? "" : l.get(0);
    }

    /** Une étape de cette écoute au journal, avec le temps depuis startListening. */
    private void note(String etape) {
        journal("écoute " + ecoute + " : " + etape + " (" + (SystemClock.elapsedRealtime() - depart) + " ms"
                + (finie ? ", déjà finie" : "") + ")");
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
        note("onReadyForSpeech");
        if (!finie) {
            ouvert = true;
            prete(ecoute);
        }
    }

    /** Parler, c'est que le micro est ouvert : certaines reconnaissances ne disent pas « prêt » d'abord. */
    @Override
    public void onBeginningOfSpeech() {
        note("onBeginningOfSpeech");
        if (!finie && !ouvert) {
            ouvert = true;
            prete(ecoute);
        }
    }

    @Override
    public void onPartialResults(Bundle partialResults) {
        partiels++;
        if (partiels == 1) {
            note("premier texte provisoire, longueur " + premier(partialResults).length());
        }
        if (!finie) {
            partiel(ecoute, premier(partialResults));
        }
    }

    @Override
    public void onResults(Bundle results) {
        ArrayList<String> l = textes(results);
        note("onResults : " + (l == null ? 0 : l.size()) + " proposition(s), longueur " + premier(results).length()
                + ", après " + partiels + " texte(s) provisoire(s) et " + niveaux + " niveau(x) du son"
                + (ouvert ? "" : ", sans « prêt »"));
        if (!finie) {
            fin();
            resultat(ecoute, premier(results));
        }
    }

    @Override
    public void onError(int error) {
        note("onError " + error + " (" + nom(error) + "), après " + partiels + " texte(s) provisoire(s) et " + niveaux
                + " niveau(x) du son" + (ouvert ? "" : ", sans « prêt »"));
        if (!finie) {
            fin();
            erreur(ecoute, error);
        }
    }

    @Override
    public void onRmsChanged(float rmsdB) {
        niveaux++;
        if (niveaux == 1) {
            note("premier niveau du son, " + rmsdB + " dB");
        }
        if (!finie) {
            niveau(ecoute, rmsdB);
        }
    }

    @Override
    public void onBufferReceived(byte[] buffer) {}

    @Override
    public void onEndOfSpeech() {
        note("onEndOfSpeech");
    }

    @Override
    public void onEvent(int eventType, Bundle params) {
        note("onEvent " + eventType);
    }

    /** Pas demandée (EXTRA_SEGMENTED_SESSION) : au journal seulement, si une reconnaissance l'envoie quand même. */
    @Override
    public void onSegmentResults(Bundle segmentResults) {
        note("onSegmentResults");
    }

    /** Une session par segments finie : plus rien n'arrivera, l'écoute finit sans texte. */
    @Override
    public void onEndOfSegmentedSession() {
        note("onEndOfSegmentedSession");
        if (!finie) {
            fin();
            resultat(ecoute, "");
        }
    }

    @Override
    public void onLanguageDetection(Bundle results) {
        note("onLanguageDetection");
    }
}
