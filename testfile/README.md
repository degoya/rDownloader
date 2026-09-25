# Legale manuelle Download-Tests

Dieses Verzeichnis enthält ausschließlich Testdaten mit nachvollziehbarer Herkunft und
Weitergabeberechtigung. Externe Hoster-Links sind manuelle Smoke-Tests und **keine** stabilen
CI-Fixtures: Hoster können Links löschen, Captchas oder Wartezeiten ändern und Multihoster
können einzelne Dienste vorübergehend deaktivieren.

Stand der externen Prüfung: **2026-09-03**.

## Lokale Testdateien

| Datei | Zweck und Herkunft | SHA-256 der lokalen Datei |
|---|---|---|
| `rdownloader-plugin-test.txt` | Eigenes, unter CC0 1.0 freigegebenes Payload für Hoster- und Multihoster-Tests | `15563d44360d595b49b4c82fdd4fd1083719f924de39e2c740287d424520387f` |
| `sabnzbd-test-download-100MB.nzb` | Snapshot der [offiziellen 100-MB-Test-NZB von SABnzbd](https://sabnzbd.org/tests/test_download_100MB.nzb); erzeugte Testdaten in `alt.binaries.test`, inklusive RAR- und PAR2-Dateien | `a2d656ede1fb102da0b2d6ff2c8e2240d1fbd96c32fd7d9e046514eb75b72709` |
| `big-buck-bunny.torrent` | [WebTorrent-Testtorrent](https://webtorrent.io/free-torrents) für den unter [CC BY 3.0](https://peach.blender.org/about/) veröffentlichten Blender-Film | `13b4241c2fc4c2be3806287895566c0f596b9716643f2e679fd1482bcc7ed449` |
| `debian-13.6.0-amd64-netinst.iso.torrent` | [Offizieller Debian-BitTorrent-Download](https://www.debian.org/CD/torrent-cd/) des amd64-Netinstallers | `763e5f84c8aff61da94f20604e078900825ab8c4d44dc66d6b9de73c5be29976` |

Die SABnzbd-Nachrichten wurden am 2025-11-22 veröffentlicht. Falls der lokale NZB-Snapshot
bei einem Usenet-Anbieter wegen dessen Retention oder Artikelabdeckung unvollständig wird,
die aktuelle Datei erneut von der oben verlinkten SABnzbd-URL laden. Die NZB lädt ungefähr
100 MB Nutzdaten beziehungsweise 115 MB yEnc-Segmente.

## Resolver-Testlinks

Alle Hoster-Zeilen sollen auf **denselben Inhalt** aus `rdownloader-plugin-test.txt` zeigen.
So lässt sich nach dem Download nicht nur „Erfolg“, sondern auch der SHA-256-Wert prüfen.

Der derzeitige eigene Test-Upload ist:

```text
https://1fichier.com/?jz57m2dq0b0liimcmimg
```

Er wurde am 2026-09-03 als anonymer 1fichier-Upload angelegt und ist dort höchstens 15 Tage
gespeichert, also spätestens am **2026-09-18 zu ersetzen**. Der Link wurde mit Dateiname
`rdownloader-plugin-test.txt` und Größe 605 B verifiziert.

### Einzelhoster

| Plugin | Free-Test | Premium-Test | Roh-Testlink |
|---|---|---|---|
| DDownload | Ohne Account; Countdown/Captcha erwarten | DDownload-Account; API-Key plus eingeloggte Browser-Cookies empfohlen | Eigener Upload erforderlich |
| FileJoker | Ohne Account; Countdown/Captcha erwarten | Cookies einer eingeloggten Premium-Sitzung | Eigener Upload erforderlich |
| KatFile | Ohne Account; Countdown/Captcha erwarten | API-Key oder Cookies einer Premium-Sitzung | Eigener Upload erforderlich |
| Keep2Share | Ohne Account; Countdown/Bild-Captcha erwarten | E-Mail und Account-Passwort | Eigener Upload erforderlich |
| Nitroflare | Ohne Account; Countdown/reCAPTCHA erwarten | E-Mail und Premium-Key | Eigener Upload erforderlich |
| 1fichier | Obigen Link ohne Account starten | Obigen Link mit dem 1fichier-API-Key starten | `https://1fichier.com/?jz57m2dq0b0liimcmimg` |
| Rapidgator | Ohne Account; Countdown/reCAPTCHA erwarten | E-Mail und Account-Passwort | Eigener Upload erforderlich |

Die sechs als „Eigener Upload erforderlich“ markierten Anbieter veröffentlichen keine
dauerhaften neutralen Roh-Testlinks. Uploads benötigen dort nach aktueller Prüfung ein
Konto beziehungsweise einen API-Key. Keine beliebigen Links aus Suchmaschinen eintragen:
Dateirechte, Inhalt und Lebensdauer wären nicht überprüfbar. Stattdessen
`rdownloader-plugin-test.txt` mit dem jeweiligen eigenen Uploader-Konto hochladen und den
erzeugten öffentlichen Link in dieser Tabelle ergänzen. Derselbe Link testet Free und
Premium; nur die in rDownloader gewählte Account-Route unterscheidet sich.

### Multihoster

Für alle Multihoster kann derselbe legale 1fichier-Link verwendet werden. 1fichier war am
Prüfdatum bei [AllDebrid](https://api.alldebrid.com/v4.1/hosts),
[Debrid-Link](https://debrid-link.com/infos/downloader),
[LinkSnappy](https://linksnappy.com/landing) und
[Premiumize.me](https://www.premiumize.me/api/services/list) gelistet. Die Verfügbarkeit
bleibt dynamisch und ist direkt vor dem Test in der Account-Hosterliste zu kontrollieren.

| Plugin | Testmodus | Testlink |
|---|---|---|
| AllDebrid | AllDebrid-Account/API-Key explizit auswählen | `https://1fichier.com/?jz57m2dq0b0liimcmimg` |
| Debrid-Link | Debrid-Link-Account/API-Key explizit auswählen | `https://1fichier.com/?jz57m2dq0b0liimcmimg` |
| LinkSnappy | LinkSnappy-Account explizit auswählen | `https://1fichier.com/?jz57m2dq0b0liimcmimg` |
| Premiumize.me | Premiumize-Account/API-Key explizit auswählen | `https://1fichier.com/?jz57m2dq0b0liimcmimg` |

Die „Test Download“-URLs auf der LinkSnappy-Statusseite sind bereits erzeugte
`dlserv*.linksnappy.com`-Direktlinks. Sie sind keine ursprünglichen Hoster-Links und eignen
sich daher weder für den DDownload-, Rapidgator-, Keep2Share-, KatFile- noch 1fichier-Resolver.

## Durchführung

1. Unter **Downloads → Download hinzufügen** den Roh-Testlink einfügen.
2. Für Free ausdrücklich **Kein Account** auswählen.
3. Für Premium den Account des Einzelhosters auswählen; für den Multihoster-Test genau den
   zu prüfenden Multihoster-Account auswählen. So entscheidet nicht die automatische
   Fallback-Reihenfolge zwischen mehreren aktivierten Accounts.
4. Bei Free-Downloads den Captcha-Dialog beantworten und die Wartezeit ablaufen lassen.
5. Nach erfolgreichem Download Dateiname, Größe (605 B) und SHA-256 prüfen:

   ```bash
   sha256sum rdownloader-plugin-test.txt
   # 15563d44360d595b49b4c82fdd4fd1083719f924de39e2c740287d424520387f
   ```

6. Für den jeweils zweiten Modus den fertigen Job entfernen oder in ein leeres Ziel laden,
   damit Deduplizierung beziehungsweise eine bereits vorhandene Datei den Test nicht abkürzt.

Ein bestandener Linkcheck allein genügt nicht: Er prüft noch nicht Countdown, Captcha,
Account-Anmeldung, Direktlink-Erzeugung, Redirects und den eigentlichen Datei-Transfer.
