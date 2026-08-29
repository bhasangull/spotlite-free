# SpotLite Free

SpotLite Free, Spotify Free hesabıyla çalışan düşük ek yüklü bir Windows oynatıcı
başlatıcısıdır. Varsayılan hafif mod Spotify'ın resmi Embed oynatıcısını, Microsoft
Edge'in tek-site uygulama modunda açar.

## Neden bu yöntem?

- Spotify Free yayın ve reklam kuralları resmi `open.spotify.com/embed` oynatıcısında kalır.
- Librespot'taki hesap-başına reddedilen eski ses anahtarı servisine bağımlı değildir.
- Web Playback SDK kullanılmaz; o SDK Premium hesap gerektirir.
- Ayrı sekmeler, uzantılar, senkronizasyon ve Edge'in arka plan modu kapatılır.
- Tek kalıcı profil sayesinde her açılışta yeniden giriş gerekmez.
- Minimal arayüz güvenilir bir web kökeni sağlamak için yalnızca yerel
  `127.0.0.1:43117` adresinde sunulur. Ses Spotify sunucularından doğrudan gelir.
- Spotify girişi PKCE ile resmi hesap sayfasında yapılır ve
  `127.0.0.1:5588/login` üzerinden uygulamaya döner. Erişim anahtarı diske yazılmaz;
  yalnızca ekranda gösterilecek hesap etiketi yerel profilde saklanır.

Bu uygulama bir tarayıcı motorunu tamamen ortadan kaldırmaz. Spotify'ın Free web
yayını DRM kullandığından desteklenen bir tarayıcı motoru zorunludur. Bu nedenle RAM
kullanımı librespot kadar düşük olmayacaktır; amaç normal, çok sekmeli bir tarayıcıdan
daha öngörülebilir ve düşük kullanım sağlamaktır.

## Kullanım

1. `spotlite-free.exe` dosyasını çalıştırın.
2. Bir Spotify playlist bağlantısı yapıştırın.
3. Hesap doğrulanmamışsa `Spotify ile giriş yap` düğmesiyle Spotify'ın resmi giriş
   sayfasında oturum açın.
4. `Başlat ve döngüye al` düğmesine basın. Son parçadan sonra playlist otomatik
   olarak baştan başlar; tek şarkılı playlistler de aynı şekilde tekrarlanır.
   Döngü düğmesi sırayla playlist döngüsü, aynı şarkı döngüsü ve kapalı durumları
   arasında geçiş yapar.
5. Sonraki açılışlarda son bağlantı ve oturum korunur. Hafif ve tam mod ayrı
   profiller kullanır; böylece ağır oynatıcı arka planda hafif moda eklenmez.

Spotify'ın kitaplık ve keşfet sayfalarını içeren ağır arayüz gerekirse uygulama
`spotlite-free.exe --full` şeklinde başlatılabilir.

## Derleme

Windows'ta güncel Rust ile:

```powershell
cargo build --release
```

Çıktı `target\release\spotlite-free.exe` olur.

## Güvenlik ve kapsam

SpotLite Free ses anahtarı çıkarmaz, DRM atlatmaz, reklam engellemez ve Spotify
içeriğini kaydetmez. Yalnızca Spotify'ın desteklediği web oynatıcısını daha küçük bir
masaüstü kabuğunda çalıştırır.
