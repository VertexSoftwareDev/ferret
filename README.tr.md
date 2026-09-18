*[English](README.md) · **Türkçe***

# Ferret

Windows için anında dosya arama. Bir harfe basın, bir milyon dosya on
milisaniyenin altında süzülsün.

Windows Search klasörlerinizi tarayıp bir veritabanı tutar; o veritabanı çoğu
zaman eskimiştir ve sık sık yavaştır. Ferret başka bir yol izler: dosya
sisteminin kendi içindekiler tablosunu — NTFS'in **ana dosya tablosunu** — ham
birimden doğrudan okur. Tek bir sıralı geçiş, ve tüm disk bellektedir.

```
  files            : 1559745
  folders          : 297737
  scan             : 6.90 s
  search index     : 0.10 s
  memory           : 163.1 MB

  query                               results       time
  a                                   1112768    15.27 ms
  exe                                   16902     4.64 ms
  setup                                  3435     6.52 ms
  windows\system32\kernel                   1     6.97 ms
  zzqqxxnope                                0     3.69 ms

  path check       : 200/200 paths exist on disk

  live updates
      created and then found       : 25 / 25
      still indexed after deletion : 0

  RESULT: passed
```

Bu, 1,8 milyon kayıtlı bir birimde `--selftest` komutunun gerçek çıktısı.
Yeniden kurulan her yolun diskte sahiden var olup olmadığını denetler; test
sürerken oluşturulan ve silinen dosyaların indekse kendiliğinden girip
çıktığını da doğrular.

## İki pencere

Ferret'in aynı motor üzerinde iki arayüzü var. İkisi de aynı işi yapar — aynı
sütunlar, aynı filtreler, aynı klavye, aynı iki dil, aynı canlı güncelleme.

**`ferret-native`** kullanılması gereken olan. Tek bir çalıştırılabilir dosya,
[egui] ile çizilmiş, arkasında tarayıcı motoru yok: dörtte bir bellek, yedi
yerine tek süreç ve yanında kurulacak hiçbir şey yok.

**`ferret-app`** aynı uygulamayı [Tauri] üzerinden bir web görünümünde çizer.
Kullanımdan kaldırılmadı, bilerek tutuluyor — yerli pencerenin vazgeçtiği bazı
şeyleri elinde tutuyor: fareyle seçilebilen yollar, tüm sistem font yığını ve
ekran okuyucuların bedava anladığı bir pencere.

Pencerenin altındaki her şey ortak, değişiklik günlüğü kurallarına varıncaya
kadar; dolayısıyla biri diğerinden sapamaz. Ölçümler ve farkların tam listesi
[docs/TWO-WINDOWS.md](docs/TWO-WINDOWS.md) içinde.

[egui]: https://github.com/emilk/egui
[Tauri]: https://tauri.app

## Ekran görüntüleri

Henüz depoya eklenmedi — kendi diskinizden alın, dosya adları orada gerçek:
uygulamayı çalıştırın, `Win+Shift+S` ile kırpın ve görüntüleri `docs/` içine
bırakın.

## Ne yapar

- **Bir NTFS birimini saniyeler içinde indeksler**, klasörleri gezerek değil
  `$MFT`'yi ayrıştırarak.
- **Siz yazarken süzer.** Her sorgu, önceden hazırlanmış küçük harfli bir
  arena üzerinde tek bir doğrusal geçiştir ve tüm çekirdeklere bölünür.
- **Yolları da arar.** `belgeler\rapor` yazın, yalnızca o klasörün eşleşmeleri
  gelsin — hem de bir milyon yol dizgesi hiç oluşturulmadan.
- **Sıralar ve filtreler**: ada, boyuta, tarihe, türe ve gizli/sistem
  özniteliklerine göre.
- **Güncel kalır.** Ferret NTFS değişiklik günlüğünü izler; uygulama açıkken
  oluşturulan, adı değiştirilen veya silinen dosyalar yaklaşık bir saniye
  içinde görünür — yeniden tarama yok.
- **Bulduğunu toplar.** Durum çubuğu eşleşmelerin toplam boyutunu taşır; boyuta
  göre yapılan aramaların ardındaki asıl soru zaten budur.
- **Bulduğunuzu açar**: çift tıklama, Enter, Gezgin'de göster, yolu kopyala,
  ya da aramayı sonucun bulunduğu klasörle sınırla.
- **Türkçe veya İngilizce konuşur**, araç çubuğundan değiştirilir ve hatırlanır.
  Yalnızca arayüz değişir: dosya adları, yollar ve tarihler diskten, dosya
  sisteminde nasıl duruyorsa öyle gelir.
- **Yalnızca okur.** Ferret birimleri okumak için açar, geri asla tek bayt
  yazmaz.

## Gereksinimler

- Windows 10 veya 11
- Bir NTFS birimi
- Yönetici izni — ham birim erişimi ayrıcalıklıdır. Ferret izinsiz başlar ve
  indekslemeye tam ihtiyaç duyduğu anda yönetici olarak yeniden başlamayı
  önerir.

## Sürümler

Etiket atmak sürecin tamamı. GitHub Actions testleri çalıştırır, her şeyi
derler ve `Ferret.exe`'yi, kurulum dosyasını ve komut satırını indirilebilir
hale getirir.

```powershell
git tag v0.1.0
git push --tags
```

## Çalıştırma

```powershell
cargo run --release -p ferret-native   # pencere
cargo run --release -p ferret-app      # aynı şey, web görünümüyle çizilmiş
```

Çalıştırılabilir dosyalar `target/release/` içine düşer: `ferret.exe`
uygulamanın kendisi, `ferret-app.exe` web görünümlü sürüm, `ferret-cli.exe` ise
aşağıdaki geliştirme aracı.

Yukarıdaki sayıların geldiği yer olan, motora ait geliştirme aracı:

```powershell
cargo run --release -p ferret-cli -- scan  C     # indeksle ve özetle
cargo run --release -p ferret-cli -- find  C rapor
cargo run --release -p ferret-cli -- bench C     # sorgu süreleri
```

Ve uçtan uca duman testi:

```powershell
cargo run --release -p ferret-native -- --selftest
```

İki çalıştırılabilir dosya da bunu sunar ve ikisi de aynı denetimleri yapar.

## Klavye

| Tuş | İşlev |
| --- | --- |
| `Ctrl+F` | Arama kutusuna odaklan |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Sonuçlar arasında gezin |
| `Enter` | Seçili öğeyi aç |
| `Ctrl+C` | Tam yolunu kopyala |
| `F5` | Geçerli sürücüyü yeniden indeksle |
| `Esc` | Sorguyu temizle |

## Nasıl çalışır

`$MFT`, dosya başına yaklaşık 1 KB'lık bir kayıt tutan bir tablodur. Her kayıt
dosyanın adını, üst klasörünün kayıt numarasını, boyutunu ve zaman damgalarını
taşır. O tabloyu baştan sona okumak, klasör ağacına tek bir soru sormadan tüm
dosya sistemini verir.

Bunu doğru yapmak, baytları sırayla okumaktan fazlasını gerektirir:

- **Fixup'lar.** NTFS, bir kayıttaki her sektörün son iki baytını bir denetim
  değeriyle değiştirir ve gerçek baytları başlıkta park eder. Bu değişimi geri
  almazsanız her 512. bayt çifti yanlış olur; denetlerseniz, yazım ortasında
  yarım kalmış kayıtları da yakalarsınız.
- **8.3 takma adları.** Çoğu dosya iki ad taşır: gerçek adı ve eski usul bir
  `PROGRA~1` takma adı. Takma adı indekslemek sonuçları çöpe çevirir.
- **Geri dönüştürülen kayıt numaraları.** Bir çocuğun kaydı, üst klasörünün
  kaydının *altında* durabilir; dolayısıyla tek bir ileri geçiş hangi girdilerin
  erişilebilir olduğuna karar veremez. Erişilebilirlik tarama bittikten sonra
  çözülür; aynı adım NTFS'in kendi iç içe meta dosyalarını da eler.
- **Arama maliyeti.** Adları her tuş vuruşunda küçük harfe çevirmek, büyük bir
  birimde sorgu başına ~100 ms tutar. Bir kez, tek bir bitişik arenaya çevirmek
  — ve o arenayı paralel aramak — 3–12 ms tutar.
- **Bayat anlık görüntüler.** Değişiklik günlüğü hangi kaydın değiştiğini
  söyler, ama o kaydı ham birimden yeniden okumak bayat baytlar döndürür: ham
  okumalar dosya sistemi önbelleğini atlar, bu yüzden bir saniye önce
  oluşturulmuş bir dosya plaka üzerinde hâlâ boştur. Doğru kaynak, günlük
  girdisinin kendi adı, üst klasörü ve öznitelikleridir.

Daha ayrıntılı anlatım [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) içinde.

## Yerleşim

```
crates/ferret-core/     NTFS okuyucu, indeks ve arama motoru (Windows API bağımlılığı yok)
crates/ferret-shell/    İndeksler, sorgular, satır biçimleme, değişiklik günlüğü izleyici
crates/ferret-native/   Pencere: egui, tek çalıştırılabilir dosya
crates/ferret-app/      Alternatif pencere: Tauri ve bir web görünümü
ui/                       onun arayüzü: HTML, CSS, JavaScript — çerçeve yok
crates/ferret-cli/      Geliştirme aracı: scan, find, bench
scripts/                İkon üreteci
```

## Durum

Çalışıyor ve gerçek birimler üzerinde ölçüldü. Bilinen sınırlar:

- Yalnızca NTFS. FAT32 ve exFAT birimleri listelenir ama indekslenemez.
- Canlı güncelleme, birimin USN günlüğünün açık olmasını gerektirir; sistem
  sürücüsünde varsayılan olarak açıktır. Açık değilse Ferret `F5`'e kadar
  anlık görüntüsünü korur.
- Bir dosyanın adı değiştiğinde yeni ad, ad arenasının sonuna eklenir ve eski
  baytlar orada kalır; yoğun değişim olan uzun bir oturumda bellek, bir sonraki
  tam taramaya kadar yavaşça büyür.
- 200 000 sonucun ötesinde sıralama atlanır: o boyutta liste zaten kimsenin
  sırayla okuduğu bir şey değildir ve sıralama gözle görülür bir duraklamaya
  mal olur.
- Yerli pencere Segoe UI ile çizer; bu font Latin, Yunan ve Kiril alfabelerini
  kapsar. Çince, Japonca, Korece veya Arapça dosya adları egui'nin kendi
  fontuna düşer ve boş kutu gösterebilir; web görünümlü pencerede tüm sistem
  font yığını olduğu için bu sorun yoktur.

## Lisans

MIT. Bkz. [LICENSE](LICENSE).
