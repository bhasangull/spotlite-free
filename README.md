# SpotLite Free

SpotLite Free, Spotify Free hesabıyla çalışan düşük ek yüklü bir Windows oynatıcı
başlatıcısıdır. Varsayılan hafif mod Spotify'ın resmi Embed oynatıcısını, Microsoft
Edge'in tek-site uygulama modunda açar.

## Neden bu yöntem?

- Spotify Free yayın ve reklam kuralları resmi `open.spotify.com/embed` oynatıcısında kalır.
- Librespot'taki hesap-başına reddedilen eski ses anahtarı servisine bağımlı değildir.
- Web Playback SDK kullanılmaz; o SDK Premium hesap gerektirir.
- Ayrı sekmeler, uzantılar, senkronizasyon ve Edge'in arka plan modu kapatılır.
- Kontrol paneli ve bütün hesaplar tek Edge işlem grubunu paylaşır. Her hesabın
  çerezleri aynı kullanıcı veri klasörü altındaki ayrı profillerde tutulur; böylece
  oturumlar karışmadan sabit tarayıcı, ağ ve ses süreçleri paylaşılır.
- Kontrol paneli açılışta Spotify iframe API'sini yüklemez; API yalnızca ilk oynatma
  isteğinde yüklenir. Medya iframe'i oluşturulduğunda `loading=eager` uygulanır;
  arka plandaki hesapların yüklenmesi için sekmeye tıklamak gerekmez.
- Minimal arayüz güvenilir bir web kökeni sağlamak için yalnızca yerel
  `127.0.0.1:43117` adresinde sunulur. Ses Spotify sunucularından doğrudan gelir.
- Spotify girişi PKCE ile resmi hesap sayfasında yapılır ve
  `127.0.0.1:5588/login` üzerinden uygulamaya döner. Erişim anahtarı diske yazılmaz;
  yenileme anahtarı Windows DPAPI ile şifrelenerek saklanır. Spotify API yetkisi ve
  web oynatıcı oturumu ayrıdır; isim sorgusunun başarısız olması oynatmayı durdurmaz.

Bu uygulama bir tarayıcı motorunu tamamen ortadan kaldırmaz. Spotify'ın Free web
yayını DRM kullandığından desteklenen bir tarayıcı motoru zorunludur. Bu nedenle RAM
kullanımı librespot kadar düşük olmayacaktır; amaç normal, çok sekmeli bir tarayıcıdan
daha öngörülebilir ve düşük kullanım sağlamaktır.

## Kullanım

1. `outputs\spotlite-free-refresh.exe` dosyasını çalıştırın (0.4.0).
2. Bir Spotify playlist bağlantısı yapıştırın.
3. Hesap doğrulanmamışsa `Spotify ile giriş yap` düğmesiyle Spotify'ın resmi giriş
   sayfasında oturum açın.
4. `Tümünde başlat` veya hesabın `Başlat` düğmesine basın. Hesap pencereleri arka
   planda, görev çubuğundan gizlenmiş olarak çalışır. `Aç` ile ilgili oynatıcı
   görülebilir; tekrar `Başlat` onu arka plana alır. Giriş penceresi yalnızca
   kullanıcının giriş işlemi için görünür olur.
5. Döngü düğmesi playlist → şarkı → kapalı sırasını izler. Döngü, gerçek Spotify
   parça sonu olayını (süre sonu veya son saniyelerden sıfıra dönüş) takip eder.
   Kısa/şüpheli medya, reklam, veri kesintisi veya ortada duraklatma zorla yeniden
   başlatılmaz. Çok parçalı playlistte son parçanın API üzerinden doğrulanması
   gerekir; doğrulanamayan listelerde Spotify'ın doğal sırası korunur. Embed API
   yerel bir `setRepeat` metodu sunmaz.
6. Sonraki açılışlarda son bağlantı ve kayıtlı çerez profili korunur. Spotify sunucusu
   oturumu iptal edebilir; sonsuz oturum garantisi yoktur. Hafif ve tam mod ayrı
   profiller kullanır; böylece ağır oynatıcı arka planda hafif moda eklenmez.

Eski, hesap başına ayrı Edge profili kullanan bir sürümden ilk geçişte birinci
hesabın oturumu otomatik taşınır. Diğer hesapların Spotify şifreleme anahtarları
birbirinden farklı olduğu için bu hesaplarda bir defalık yeniden giriş gerekebilir.
Eski profil klasörleri geçiş sırasında silinmez.

Spotify'ın kitaplık ve keşfet sayfalarını içeren ağır arayüz gerekirse uygulama
`spotlite-free-refresh.exe --full` şeklinde başlatılabilir.

Yeni hesaplar için isteğe bağlı ayrı IPv6 çıkışı `work/audit-2026-09-13/ipv6-routing.md`
dosyasında açıklanır. Bu özellik bilgisayara atanmış genel IPv6 adresleri gerektirir.

## Sessiz arka plan

SpotLite'ın kendine ait Edge kullanıcı veri dizini `--mute-audio` ile başlatılır.
Bu ayar bilgisayarın veya diğer tarayıcı profillerinin sesini değiştirmez; reklamlar
dahil bu oynatıcının yerel ses çıkışını kapatır. Medya yetkileri/DRM değiştirilmez.

Hesap pencereleri küçültülmez veya 1×1 yapılmaz: normal boyutta ekran dışında ve
görev çubuğundan gizli tutulur. Yalnızca SpotLite tarayıcı sürecinde arka plan
zamanlayıcı ve pencere örtülmesi kısıtlamaları kapatılır. Bu tercih, uyuyan sekmelerden
daha fazla kaynak kullanabilir; kesintisiz arka plan oynatmaya öncelik verir.

`/status` içindeki `health` ve `window` alanları gerçek odak, konum, iframe hazırlığı
ve tekrar sayısını gösterir. Bu sağlık bilgileri `playing` veya medya zaman damgasını
güncellemez. Güncel Spotify verisi gelmezse panel “çalıyor” göstermeyi bırakır.

## Derleme

Windows'ta güncel Rust ile:

```powershell
cargo test --locked
node --test tests/playback.test.cjs tests/player-driver.test.cjs
./build-release.ps1
```

Dağıtım dosyası yalnızca `outputs\spotlite-free-refresh.exe` olur. Arka planda gerçek
oynatmayı salt okunur denetlemek için `node tests/live-background.cjs 330` kullanılabilir.
Bu test hiçbir başlatma veya yeniden başlatma komutu göndermez.

## Güvenlik ve kapsam

SpotLite Free ses anahtarı çıkarmaz, DRM atlatmaz, reklam engellemez ve Spotify
içeriğini kaydetmez. Yalnızca Spotify'ın desteklediği web oynatıcısını daha küçük bir
masaüstü kabuğunda çalıştırır.
