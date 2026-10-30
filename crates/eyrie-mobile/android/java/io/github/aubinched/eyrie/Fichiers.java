package io.github.aubinched.eyrie;

import android.app.Activity;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Matrix;
import android.media.ExifInterface;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;

/**
 * Le choix d'un fichier du téléphone, pour la saisie d'un terminal du PC (Eyrie l'envoie au PC, qui rend son chemin) :
 * une activité sans vue qui ouvre le sélecteur d'Android (photos, fichiers, Drive…), lit le fichier choisi et le rend
 * au code natif. NativeActivity ne reçoit pas les résultats d'activité : il lui faut cette activité à elle.
 *
 * <p>Une image de plus de 3 Mo est réduite (2048 px de côté au plus, JPEG, tournée selon son EXIF) : une photo d'un
 * capteur de 50 Mpx n'a pas à traverser le réseau, et Claude la réduirait de toute façon. Un autre fichier passe tel
 * quel jusqu'à 10 Mo ; au-delà, il est refusé.
 */
public final class Fichiers extends Activity {
    /** Le plus gros fichier envoyé (comme `UPLOAD_MAX` du protocole). */
    static final int MAX = 10 * 1024 * 1024;
    /** Au-delà, une image est réduite. */
    static final int IMAGE_RAW = 3 * 1024 * 1024;
    /** Le plus grand côté d'une image réduite. */
    static final int IMAGE_SIDE = 2048;
    /** Une image lue avant d'être réduite, au plus. */
    static final int IMAGE_READ = 64 * 1024 * 1024;

    /** Le fichier choisi : son nom et son contenu. */
    static native void recu(String nom, byte[] donnees);

    /** Rien n'a été choisi. */
    static native void rien();

    /** Le fichier choisi n'a pas pu être lu, ou est trop gros. */
    static native void echec(String pourquoi);

    /** Ouvre le sélecteur, depuis l'activité de l'application. */
    public static void choisir(Context c) {
        c.startActivity(new Intent(c, Fichiers.class));
    }

    private boolean ouvert;

    @Override
    protected void onCreate(Bundle etat) {
        super.onCreate(etat);
        // Recréée (l'écran a tourné) : le sélecteur est déjà ouvert, son résultat viendra.
        if (etat != null && etat.getBoolean("ouvert")) {
            ouvert = true;
            return;
        }
        Intent pick = new Intent(Intent.ACTION_GET_CONTENT);
        pick.setType("*/*");
        pick.addCategory(Intent.CATEGORY_OPENABLE);
        try {
            startActivityForResult(Intent.createChooser(pick, "Joindre un fichier"), 1);
            ouvert = true;
        } catch (Exception e) {
            echec("Pas de sélecteur de fichiers sur ce téléphone.");
            finish();
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle etat) {
        super.onSaveInstanceState(etat);
        etat.putBoolean("ouvert", ouvert);
    }

    @Override
    protected void onActivityResult(int demande, int resultat, Intent donnees) {
        super.onActivityResult(demande, resultat, donnees);
        final Uri uri = resultat == RESULT_OK && donnees != null ? donnees.getData() : null;
        if (uri == null) {
            rien();
            finish();
            return;
        }
        // Lu avant de fermer l'activité (le droit de lire le fichier lui est donné), dans un fil : un gros fichier ou une
        // image à réduire ne gèle pas l'écran.
        final ContentResolver cr = getContentResolver();
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    lire(cr, uri);
                } catch (Throwable t) {
                    echec("Fichier illisible : " + t.getMessage());
                }
                runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        finish();
                    }
                });
            }
        }).start();
    }

    private static void lire(ContentResolver cr, Uri uri) throws Exception {
        String nom = nom(cr, uri);
        String type = cr.getType(uri);
        boolean image = type != null && type.startsWith("image/") && !type.equals("image/gif");
        byte[] octets = octets(cr, uri, image ? IMAGE_READ : MAX);
        if (octets == null) {
            echec("Fichier trop gros : 10 Mo au plus.");
            return;
        }
        if (image && octets.length > IMAGE_RAW) {
            byte[] reduite = reduire(octets);
            if (reduite != null) {
                octets = reduite;
                int point = nom.lastIndexOf('.');
                nom = (point > 0 ? nom.substring(0, point) : nom) + ".jpg";
            }
        }
        if (octets.length > MAX) {
            echec("Fichier trop gros : 10 Mo au plus.");
            return;
        }
        recu(nom, octets);
    }

    /** Le nom du fichier, tel que le fournisseur le donne (sinon le dernier morceau de son adresse). */
    private static String nom(ContentResolver cr, Uri uri) {
        String nom = null;
        try (Cursor c = cr.query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                nom = c.getString(0);
            }
        } catch (Exception e) {
            nom = null;
        }
        if (nom == null || nom.isEmpty()) {
            nom = uri.getLastPathSegment();
        }
        return nom == null || nom.isEmpty() ? "fichier" : nom;
    }

    /** Le contenu, `null` au-delà de `max` octets. */
    private static byte[] octets(ContentResolver cr, Uri uri, int max) throws Exception {
        try (InputStream in = cr.openInputStream(uri)) {
            if (in == null) {
                throw new Exception("pas de contenu");
            }
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] morceau = new byte[64 * 1024];
            int lu;
            while ((lu = in.read(morceau)) != -1) {
                if (out.size() + lu > max) {
                    return null;
                }
                out.write(morceau, 0, lu);
            }
            return out.toByteArray();
        }
    }

    /** L'image réduite à `IMAGE_SIDE` de côté, en JPEG, tournée selon son EXIF ; `null` si elle ne se décode pas. */
    private static byte[] reduire(byte[] octets) {
        BitmapFactory.Options bornes = new BitmapFactory.Options();
        bornes.inJustDecodeBounds = true;
        BitmapFactory.decodeByteArray(octets, 0, octets.length, bornes);
        int cote = Math.max(bornes.outWidth, bornes.outHeight);
        if (cote <= 0) {
            return null;
        }
        BitmapFactory.Options opts = new BitmapFactory.Options();
        opts.inSampleSize = 1;
        while (cote / (opts.inSampleSize * 2) >= IMAGE_SIDE) {
            opts.inSampleSize *= 2;
        }
        Bitmap b = BitmapFactory.decodeByteArray(octets, 0, octets.length, opts);
        if (b == null) {
            return null;
        }
        float echelle = Math.min(1f, (float) IMAGE_SIDE / Math.max(b.getWidth(), b.getHeight()));
        Matrix m = new Matrix();
        m.postScale(echelle, echelle);
        m.postRotate(rotation(octets));
        Bitmap r = Bitmap.createBitmap(b, 0, 0, b.getWidth(), b.getHeight(), m, true);
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        r.compress(Bitmap.CompressFormat.JPEG, 85, out);
        return out.toByteArray();
    }

    /** La rotation que demande l'EXIF d'une photo (0 sans EXIF). */
    private static int rotation(byte[] octets) {
        try {
            ExifInterface exif = new ExifInterface(new ByteArrayInputStream(octets));
            switch (exif.getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
                case ExifInterface.ORIENTATION_ROTATE_90:
                    return 90;
                case ExifInterface.ORIENTATION_ROTATE_180:
                    return 180;
                case ExifInterface.ORIENTATION_ROTATE_270:
                    return 270;
                default:
                    return 0;
            }
        } catch (Exception e) {
            return 0;
        }
    }
}
