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

    private final Context ctx;
    private final int ecoute;
    private final SpeechRecognizer mien;
    /** La reconnaissance du téléphone lui-même (pas son service par défaut). */
    private final boolean surAppareil;
    private boolean finie;
    /**
     * Pour le journal : l'instant de startListening, ce qui est déjà arrivé, le son le plus fort (dB) ; le micro s'est
     * dit ouvert.
     */
    private long depart;
    private int partiels;
    private int niveaux;
    private float plusFort = -100f;
    private boolean ouvert;

    private Dictee(Context ctx, int ecoute, SpeechRecognizer mien, boolean surAppareil) {
        this.ctx = ctx;
        this.ecoute = ecoute;
        this.mien = mien;
        this.surAppareil = surAppareil;
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
            exception("service par défaut, réglage voice_recognition_service", e);
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
            exception("service par défaut, nom de " + paquet, e);
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
            exception("composant de la reconnaissance du téléphone", e);
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
                    Dictee d = new Dictee(ctx, ecoute, reconnaisseur, appareil);
                    reconnaisseur.setRecognitionListener(d);
                    journal("écoute " + ecoute + " : setRecognitionListener");
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
     * le plus souvent) ; ce qu'il en devient part à `modele`, une seule fois. Avant, ce que la reconnaissance a déjà va
     * au journal, et décide : déjà installée, déjà en cours, ou pas proposée, rien n'est demandé.
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
                Langues.demander(ctx, true);
            }
        });
    }

    /**
     * Après une erreur de langue de la reconnaissance du téléphone, ce qu'elle a déjà et ce qu'elle propose, au journal
     * seulement (Android 13 et suivants) : « ne connaît pas » (12) se vérifie là.
     */
    private static void journalDesLangues(final Context ctx) {
        if (Build.VERSION.SDK_INT < 33) {
            return;
        }
        PRINCIPAL.post(new Runnable() {
            @Override
            public void run() {
                Langues.demander(ctx, false);
            }
        });
    }

    /**
     * Les langues de la reconnaissance du téléphone (Android 13 et suivants) : ce qu'elle a et propose
     * (checkRecognitionSupport), puis, pour `telecharger`, le téléchargement (triggerModelDownload). Sa reconnaissance a
     * son RecognitionListener : sans lui, Android tait ce qui empêche la liaison au service (ERROR_CLIENT, trop de
     * demandes), et rien ne répondrait. Sans réponse en 30 s avant le téléchargement, abandonné. Une seule fin
     * (`dire`) ; tout se passe dans le fil principal.
     */
    private static final class Langues implements RecognitionListener {
        private static final long REPONSE_EN = 30_000;
        /** Après un téléchargement demandé sans suivi : la liaison reste le temps qu'il commence. */
        private static final long LIAISON_GARDEE = 120_000;

        private final Context ctx;
        private final boolean telecharger;
        private final String langue;
        private final Intent intention;
        private SpeechRecognizer r;
        /** Fini, dit à Rust (pour `telecharger`) ; triggerModelDownload appelé. */
        private boolean dit;
        private boolean declenche;
        private final Runnable veille = new Runnable() {
            @Override
            public void run() {
                if (!dit && !declenche) {
                    journal(quoi() + " : pas de réponse de la reconnaissance en " + REPONSE_EN / 1000 + " s");
                    dire(2, -1);
                }
            }
        };

        private Langues(Context ctx, boolean telecharger) {
            this.ctx = ctx;
            this.telecharger = telecharger;
            this.langue = langue();
            this.intention = intention(ctx, langue);
        }

        static void demander(Context ctx, boolean telecharger) {
            Langues l = new Langues(ctx, telecharger);
            journal((telecharger ? "téléchargement de " + l.langue + " demandé"
                    : "langues du téléphone demandées, pour " + l.langue) + " (" + composantSurAppareil() + ")");
            try {
                l.r = SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx);
                l.r.setRecognitionListener(l);
            } catch (Throwable e) {
                exception(l.quoi() + ", création", e);
                l.dire(2, -1);
                return;
            }
            PRINCIPAL.postDelayed(l.veille, REPONSE_EN);
            // Un appel après l'autre : deux appels avant la liaison au service en feraient deux.
            try {
                l.r.checkRecognitionSupport(l.intention, ctx.getMainExecutor(), new RecognitionSupportCallback() {
                    @Override
                    public void onSupportResult(RecognitionSupport s) {
                        l.langues(s);
                    }

                    @Override
                    public void onError(int code) {
                        journal("langues du téléphone : erreur " + code + " (" + nom(code) + ")");
                        l.ensuite();
                    }
                });
            } catch (Throwable e) {
                exception(l.quoi() + ", checkRecognitionSupport", e);
                l.ensuite();
            }
        }

        private String quoi() {
            return telecharger ? "téléchargement" : "langues du téléphone";
        }

        /** Ce que la reconnaissance a et propose : installée, en cours, pas proposée, rien à demander. */
        private void langues(RecognitionSupport s) {
            List<String> installees = s.getInstalledOnDeviceLanguages();
            List<String> enCours = s.getPendingOnDeviceLanguages();
            List<String> proposees = s.getSupportedOnDeviceLanguages();
            journal("langues du téléphone : installées " + installees + ", en cours " + enCours
                    + ", téléchargeables " + proposees + ", en ligne " + s.getOnlineLanguages());
            if (dit || !telecharger) {
                dire(2, 0);
            } else if (parmi(installees, langue)) {
                journal("téléchargement : " + langue + " déjà installée");
                dire(0, 0);
            } else if (parmi(enCours, langue)) {
                journal("téléchargement : " + langue + " déjà en cours");
                dire(1, 0);
            } else if (proposees != null && !proposees.isEmpty() && !parmi(proposees, langue)) {
                // Des langues proposées, pas la sienne : un téléchargement n'y ferait rien.
                journal("téléchargement : " + langue + " pas proposée");
                dire(2, SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED);
            } else {
                ensuite();
            }
        }

        /** Les langues lues (ou pas) : le téléchargement, pour `telecharger` ; sinon, fini. */
        private void ensuite() {
            if (telecharger) {
                declencher();
            } else {
                dire(2, 0);
            }
        }

        /** Le téléchargement lui-même. */
        private void declencher() {
            if (dit || declenche) {
                return;
            }
            declenche = true;
            PRINCIPAL.removeCallbacks(veille);
            try {
                if (Build.VERSION.SDK_INT >= 34) {
                    r.triggerModelDownload(intention, ctx.getMainExecutor(), new ModelDownloadListener() {
                        private int dizaine = -1;

                        @Override
                        public void onProgress(int pourcent) {
                            // Au journal de dix en dix.
                            if (pourcent / 10 != dizaine) {
                                dizaine = pourcent / 10;
                                journal("téléchargement : " + pourcent + " %");
                            }
                        }

                        @Override
                        public void onSuccess() {
                            journal("téléchargement : fini");
                            dire(0, 0);
                        }

                        @Override
                        public void onScheduled() {
                            journal("téléchargement : prévu plus tard par Android");
                            dire(1, 0);
                        }

                        @Override
                        public void onError(int code) {
                            journal("téléchargement : erreur " + code + " (" + nom(code) + ")");
                            if (code == SpeechRecognizer.ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS) {
                                // Le service ne sait télécharger que sans écouteur (celui qui en prend un, sans le
                                // savoir, répond seulement 15) : redemandé ainsi.
                                sansSuivi("sans suivi, redemandé sans écouteur");
                            } else {
                                dire(2, code);
                            }
                        }
                    });
                } else {
                    sansSuivi("demandé (Android 13, sans suivi)");
                }
            } catch (Throwable e) {
                exception("téléchargement, triggerModelDownload", e);
                dire(2, -1);
            }
        }

        /** Demandé sans écouteur : Android n'en dira pas la fin (15) ; la liaison reste le temps qu'il commence. */
        private void sansSuivi(String etape) {
            if (dit) {
                return;
            }
            try {
                r.triggerModelDownload(intention);
            } catch (Throwable e) {
                exception("téléchargement, triggerModelDownload sans écouteur", e);
                dire(2, -1);
                return;
            }
            journal("téléchargement : " + etape);
            dit = true;
            PRINCIPAL.removeCallbacks(veille);
            final SpeechRecognizer gardee = r;
            PRINCIPAL.postDelayed(new Runnable() {
                @Override
                public void run() {
                    detruire(gardee);
                }
            }, LIAISON_GARDEE);
            modele(2, SpeechRecognizer.ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS);
        }

        /** La fin, une seule fois : la reconnaissance détruite, et, pour `telecharger`, ce qu'il en devient à Rust. */
        private void dire(int etat, int code) {
            if (dit) {
                return;
            }
            dit = true;
            PRINCIPAL.removeCallbacks(veille);
            if (r != null) {
                detruire(r);
            }
            if (telecharger) {
                modele(etat, code);
            }
        }

        private void detruire(SpeechRecognizer r) {
            try {
                r.destroy();
            } catch (Throwable e) {
                exception(quoi() + ", destroy", e);
            }
        }

        /** Ce que la liaison au service ne peut faire (ERROR_CLIENT, trop de demandes…) : la fin. */
        @Override
        public void onError(int code) {
            journal(quoi() + " : erreur de la reconnaissance " + code + " (" + nom(code) + ")"
                    + (dit ? ", déjà fini" : ""));
            dire(2, code);
        }

        @Override
        public void onReadyForSpeech(Bundle params) {}

        @Override
        public void onBeginningOfSpeech() {}

        @Override
        public void onRmsChanged(float rmsdB) {}

        @Override
        public void onBufferReceived(byte[] buffer) {}

        @Override
        public void onEndOfSpeech() {}

        @Override
        public void onResults(Bundle results) {}

        @Override
        public void onPartialResults(Bundle partialResults) {}

        @Override
        public void onEvent(int eventType, Bundle params) {}
    }

    /** La langue `tag` (« fr-FR ») est-elle dans la liste, telle quelle ou sans région (« fr ») ? */
    private static boolean parmi(List<String> liste, String tag) {
        if (liste == null) {
            return false;
        }
        String seule = tag.split("-")[0];
        for (String l : liste) {
            String x = l == null ? "" : l.replace('_', '-');
            if (x.equalsIgnoreCase(tag) || x.equalsIgnoreCase(seule)) {
                return true;
            }
        }
        return false;
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

    /**
     * Ce qui est arrivé de cette écoute, pour le journal d'une fin. Le son le plus fort dit si le micro a capté quelque
     * chose : vers -2 dB, rien que du silence (le micro coupé par Android ou le fabricant, ou personne n'a parlé).
     */
    private String bilan() {
        return "après " + partiels + " texte(s) provisoire(s) et " + niveaux + " niveau(x) du son"
                + (niveaux > 0 ? ", le plus fort " + plusFort + " dB" : "") + (ouvert ? "" : ", sans « prêt »");
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
                + ", " + bilan());
        if (!finie) {
            fin();
            resultat(ecoute, premier(results));
        }
    }

    @Override
    public void onError(int error) {
        note("onError " + error + " (" + nom(error) + "), " + bilan());
        if (!finie) {
            fin();
            erreur(ecoute, error);
            // La langue : ce que la reconnaissance du téléphone a et propose, au journal (après la fermeture de
            // celle-ci, postée par `fin`).
            if (surAppareil && (error == SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED
                    || error == SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE)) {
                journalDesLangues(ctx);
            }
        }
    }

    @Override
    public void onRmsChanged(float rmsdB) {
        niveaux++;
        plusFort = Math.max(plusFort, rmsdB);
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
        note("onEndOfSegmentedSession, " + bilan());
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
