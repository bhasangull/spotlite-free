# SpotLite

SpotLite, düşük kaynak kullanımını ve kişiselleştirilebilir bir müzik deneyimini hedefleyen bağımsız bir Rust projesidir.

## Prototip durumu

Prototipte basit bir yerel arayüz, mevcut Soloist'e bağlantı, Spotify Web API girişi ve playlist listesinin ilk sayfası bulunur. Yerel MP3 dosyaları seçilip çalınabilir. Vurgu rengi ve arka plan seçilebilir; ayarlar açıkça kaydedilebilir. Soloist kurulumu, kalıcı giriş ve son arayüz tasarımı henüz tamamlanmadı.

```powershell
cargo test
cargo run
cargo run -- status "SOLOIST_VERI_KLASORU"
cargo run -- play "SOLOIST_VERI_KLASORU" "https://open.spotify.com/track/..."
cargo run -- local "sarki.mp3"
cargo run -- library "SPOTIFY_CLIENT_ID"
```

Windows'ta `cargo build --release --locked` komutuyla konsolsuz arayüz deneme sürümü `target/release/spotlite.exe` altında hazırlanır. Yukarıdaki komut satırı çıktıları geliştirme derlemesi (`cargo run`) içindir.

Soloist için kendi Spotify anahtarınız ve WebSocket API'si etkin çalışan bir kurulum gerekir. `status` komutu yalnızca yerel bağlantı üzerinden okuma yapar ve oynatma komutu göndermez. Soloist ayrı indirilir; bu depoya dahil edilmez. Spotify kurulumu ve kullanım koşulları için [resmî belgelere](https://developer.spotify.com/documentation/soloist) bakın.

Durum okuma, gerçek bir çalışan Soloist oturumunda test edildi. Yerel oynatma, Windows'ta kısa ve sessiz bir MP3 test dosyasıyla doğrulandı. Spotify kitaplığının ilk sayfası gerçek bir hesapla okundu. Komut gönderimi yalnızca yerel test sunucusunda sınandı; Soloist'in komut kabulü, oynatmanın tamamlandığını tek başına garanti etmez.

Arayüzde Soloist bağlantısı için `ws.port` ve `ws.addr` dosyalarının bulunduğu klasör seçilir. MP3 çalmak Soloist'i otomatik durdurmaz; aynı anda iki kaynak duyulabilir. Bu, prototipin bilinen sınırlamasıdır.

Görünüm bölümündeki kaydet düğmesi vurgu rengini, arka plan dosyasının yolunu, herkese açık Client ID'yi ve Soloist klasörünü Windows kullanıcı profiline kaydeder. Spotify parolası ve erişim anahtarı kaydedilmez. Seçilen görselin dosyası yerinde kalmalı; bellek tüketimini sınırlamak için en fazla 12 MB dosya kabul edilir ve görsel küçültülerek kullanılır.

`library` komutu için Spotify geliştirici uygulamanızda `http://127.0.0.1:8989/login` yönlendirme adresi tanımlı olmalıdır. Giriş Spotify'ın kendi sayfasında yapılır; şifre alınmaz. Bu ilk prototip giriş anahtarını diske kaydetmez, dolayısıyla komut her çalıştırıldığında yeniden yetkilendirme gerekebilir. Spotify'ın API kuralları hangi listelere ve içeriklerine erişilebileceğini sınırlayabilir.

Mimari ve protokol hakkında [Soloist WebSocket belgeleri](https://developer.spotify.com/documentation/soloist/reference/websocket-api) incelenmiştir. Bu kod, Spotifast uygulamasından kopyalanmamıştır.
