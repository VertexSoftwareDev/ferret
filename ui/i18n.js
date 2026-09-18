/*
 * Interface text.
 *
 * Only the chrome is translated. File names, paths, sizes and dates come from
 * the disk and are shown exactly as the filesystem has them — a Turkish
 * interface does not rename anyone's files.
 *
 * Errors cross from Rust as a code, optionally followed by `:detail`, so that a
 * message raised while the window was in Turkish still reads correctly after
 * switching to English.
 */

'use strict';

const I18N = {
  tr: {
    locale: 'tr-TR',

    searchPlaceholder: 'Dosya adı ara…   (yol için:  belgeler\\rapor)',
    searchLabel: 'Arama',
    driveLabel: 'Sürücü',
    languageLabel: 'Dil',
    rescanTitle: 'Yeniden tara (F5)',
    themeTitle: 'Tema değiştir',

    filesOnly: 'Yalnızca dosya',
    dirsOnly: 'Yalnızca klasör',
    skipHidden: 'Gizlileri atla',
    atLeast: 'En az',

    colName: 'Ad',
    colPath: 'Konum',
    colKind: 'Tür',
    colSize: 'Boyut',
    colDate: 'Değiştirilme',
    folder: 'Klasör',

    menuOpen: 'Aç',
    menuReveal: 'Klasörde göster',
    menuCopy: 'Yolu kopyala',

    preparing: 'Hazırlanıyor…',
    scanning: (letter) => `${letter}: taranıyor`,
    scanningDetail:
      'Ana dosya tablosu okunuyor. Bu, diskin tamamı için birkaç saniye sürer.',
    scanFailed: 'Taranamadı',
    startFailed: 'Başlatılamadı',
    retry: 'Tekrar dene',

    elevationTitle: 'Yönetici izni gerekiyor',
    elevationDetail:
      'Ferret, diski doğrudan okuyarak indeksliyor; Windows bunun için yönetici izni istiyor. ' +
      'Diske yalnızca okuma yapılır, hiçbir şey yazılmaz.',
    elevationAction: 'Yönetici olarak yeniden başlat',

    noVolumeTitle: 'NTFS sürücüsü bulunamadı',
    noVolumeDetail: 'Ferret yalnızca NTFS birimlerini okuyabilir.',

    emptyTitle: (query) => `“${query}” için sonuç yok`,
    // Ferret accepts either slash in a path query, and the forward one keeps
    // this string free of escapes.
    emptyHint:
      'Yazımı kontrol et, ya da filtreleri gevşet. Bir klasörün içinde aramak için eğik çizgi kullan: belgeler/rapor',
    emptyFiltered: 'Filtreler bu sorguda her şeyi eledi.',

    sortSkipped: 'Sonuç kümesi çok büyük olduğu için sıralama uygulanmadı.',
    indexStale: 'Disk çok değişti; güncel sonuçlar için F5 ile yeniden tara.',
    clipboardFailed: (path) => `Pano kullanılamadı: ${path}`,

    results: (count) => `${count} sonuç`,
    timing: (ms) => `${ms} ms`,
    volumeLine: (letter, files, dirs, seconds, memory) =>
      `${letter}:  ${files} dosya · ${dirs} klasör · ${seconds} sn'de tarandı · ${memory}  ·  canlı`,
    titleVolume: (letter, files) => `Ferret — ${letter}: ${files} dosya`,
    titleResults: (count) => `Ferret — ${count} sonuç`,

    errors: {
      no_backend: 'Ferret arka ucu bulunamadı.',
      no_drive_letter: 'Sürücü seçilmedi.',
      needs_elevation: 'Erişim reddedildi. Yönetici izni gerekiyor.',
      drive_not_found: 'Sürücü bulunamadı.',
      not_indexed: (letter) => `${letter}: henüz taranmadı.`,
      scan_failed: (detail) => `Taranamadı: ${detail}`,
      open_failed: (detail) => `Dosya açılamadı: ${detail}`,
      reveal_failed: (detail) => `Klasör açılamadı: ${detail}`,
      restart_failed: (detail) => `Yeniden başlatılamadı: ${detail}`,
    },
  },

  en: {
    locale: 'en-GB',

    searchPlaceholder: 'Search file names…   (for a path:  documents\\report)',
    searchLabel: 'Search',
    driveLabel: 'Drive',
    languageLabel: 'Language',
    rescanTitle: 'Re-index (F5)',
    themeTitle: 'Switch theme',

    filesOnly: 'Files only',
    dirsOnly: 'Folders only',
    skipHidden: 'Skip hidden',
    atLeast: 'At least',

    colName: 'Name',
    colPath: 'Location',
    colKind: 'Type',
    colSize: 'Size',
    colDate: 'Modified',
    folder: 'Folder',

    menuOpen: 'Open',
    menuReveal: 'Show in folder',
    menuCopy: 'Copy path',

    preparing: 'Getting ready…',
    scanning: (letter) => `Indexing ${letter}:`,
    scanningDetail:
      'Reading the master file table. A whole disk takes a few seconds.',
    scanFailed: 'Could not index',
    startFailed: 'Could not start',
    retry: 'Try again',

    elevationTitle: 'Administrator rights needed',
    elevationDetail:
      'Ferret indexes by reading the disk directly, and Windows requires administrator rights for that. ' +
      'The disk is only ever read, never written to.',
    elevationAction: 'Restart as administrator',

    noVolumeTitle: 'No NTFS drive found',
    noVolumeDetail: 'Ferret can only read NTFS volumes.',

    emptyTitle: (query) => `No results for “${query}”`,
    emptyHint:
      'Check the spelling, or loosen the filters. To search inside a folder, use a slash: documents/report',
    emptyFiltered: 'The filters ruled out everything this query matched.',

    sortSkipped: 'Too many results to sort, so the order is as found.',
    indexStale: 'The disk has changed a lot; press F5 to re-index.',
    clipboardFailed: (path) => `Clipboard unavailable: ${path}`,

    results: (count) => `${count} results`,
    timing: (ms) => `${ms} ms`,
    volumeLine: (letter, files, dirs, seconds, memory) =>
      `${letter}:  ${files} files · ${dirs} folders · indexed in ${seconds} s · ${memory}  ·  live`,
    titleVolume: (letter, files) => `Ferret — ${letter}: ${files} files`,
    titleResults: (count) => `Ferret — ${count} results`,

    errors: {
      no_backend: 'The Ferret backend is not available.',
      no_drive_letter: 'No drive selected.',
      needs_elevation: 'Access denied. Administrator rights are needed.',
      drive_not_found: 'Drive not found.',
      not_indexed: (letter) => `${letter}: not indexed yet.`,
      scan_failed: (detail) => `Could not index: ${detail}`,
      open_failed: (detail) => `Could not open the file: ${detail}`,
      reveal_failed: (detail) => `Could not open the folder: ${detail}`,
      restart_failed: (detail) => `Could not restart: ${detail}`,
    },
  },
};

const LANGUAGE_KEY = 'ferret-language';

/** The language to start in: the stored choice, else the system's, else English. */
function initialLanguage() {
  try {
    const stored = localStorage.getItem(LANGUAGE_KEY);
    if (stored && I18N[stored]) return stored;
  } catch {
    /* storage can be blocked; fall through to the system language */
  }
  return (navigator.language || '').toLowerCase().startsWith('tr') ? 'tr' : 'en';
}

function rememberLanguage(code) {
  try {
    localStorage.setItem(LANGUAGE_KEY, code);
  } catch {
    /* the choice still applies for this session */
  }
}
