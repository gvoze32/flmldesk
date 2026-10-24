# FL ML Desk

Aplikasi desktop (Rust + egui) untuk mengedit save Master League (`ML000000xx`)
**Football Life 2026** dan **PES 2021** yang sedang berjalan:

- **Rekrut pemain ke klubmu** dengan fee, gaji per tahun dan tanggal akhir
  kontrak pilihanmu. Fee dipotong dari transfer budget, dan kontrak langsung
  berstatus resmi.
- **Jual pemain dari klubmu**: kontraknya dihapus dan fee masuk ke transfer budget.
- **Transfer antar klub AI** (hanya mengubah skuad).
- **Edit budget**: transfer budget, salary budget tahunan, nama manajer.
- **Cek update otomatis** dari GitHub Releases.

Satu file program, tanpa instalasi Python. Ada versi Windows, macOS dan Linux di
[Releases](../../releases).

## Cara pakai

1. Di game, simpan Master League lalu **keluar ke menu utama atau tutup game**.
2. Jalankan `FLMLDesk.exe`.
3. **Buka save ML…**, atau seret file save ke jendela aplikasi. Lokasi save:
   `Documents\KONAMI\eFootball PES 2021 SEASON UPDATE\<folder>\save\`.
   - Klub yang kamu latih dan tanggal game dibaca otomatis dari save.
   - Kalau ada `EDIT00000000` di folder yang sama, nama pemainnya ikut dimuat.
4. **Folder game (nama/OVR)…** (cukup sekali): pilih folder instalasi game, yang
   berisi folder `download`. Nama, posisi, umur dan OVR semua pemain dibaca dari
   `Player.bin` di file `.cpk`.
5. Tab **Transfer pemain**: cari pemain, isi **Fee transfer**, **Gaji per tahun**
   dan **Kontrak s.d.**, lalu tekan **Transfer →**.
   - Angka bisa ditulis `12.500.000`, `12,5jt`, `750rb`, `3m` atau `500k`.
   - Kalau pemain yang dipilih ada di klubmu, mode berubah menjadi **jual**.
6. Tab **Budget & manajer**: ubah angka lalu tekan **Terapkan**.
7. **Simpan ke save (backup otomatis)**. File lama disalin dulu ke
   `FLMLDesk_backup\` di samping save. Setelah itu load save di game.

Semua perubahan bisa dibatalkan satu per satu sebelum disimpan. **Mata uang**
hanya mengubah tampilan dan input, memakai kurs yang sama dengan game
(misalnya ¥ = €×125). Save selalu menyimpan dalam satuan €100.

## Apa yang diubah di save

Semua offset di bawah relatif ke blok `data` setelah didekripsi. Status
"terbukti" artinya sudah dicocokkan dengan save FL26 asli: save sebelum deal,
setelah deal disepakati, dan setelah pemain datang, ditambah angka yang tampil
di game.

| Data | Lokasi | Status |
|---|---|---|
| Skuad klub (40 slot) | `0x50 + klub·0x690`, roster `+0x14C`, jumlah `+0x426` | Terbukti |
| Transfer budget | FL26 `0xC7DD00` · PES21 `0xC7DE00`, u32 × €100 | FL26 terbukti (¥307.001.287.500 = 24.560.103 × 12.500); PES21 dari Money Tool |
| Salary budget tahunan | budget `+0x14`, u32 × €100 | Terbukti. Game menampilkan nilai ini dikurangi total gaji |
| Nama manajer | `0x1942C0`, 46 byte UTF-8 | Terbukti |
| Tabel kontrak klubmu | baris 48 byte, dicari lewat skuad | Terbukti, lihat di bawah |

Isi satu baris kontrak (12 × u32):

| Word | Isi |
|---|---|
| w0, w1 | internal id, player id |
| w2 | gaji per tahun (× €100) |
| w3 | nilai pemain saat direkrut (× €100) |
| w4 | akhir kontrak (`thn \| bln<<16 \| tgl<<24`) |
| w5 | indeks baris (16 bit bawah) |
| w6 | referensi klub (14 bit bawah = indeks klub) |
| w7 | status: 5 = resmi, 4 = deal disepakati, 1/3 = negosiasi |
| w8 | tanggal mulai kontrak |

Yang dilakukan game saat merekrut, dan ditiru editor:

- Fee dipotong dari transfer budget **saat deal disepakati**. Di save contoh,
  transfer budget turun persis sebesar jumlah 3 fee.
- Salary budget tersimpan **tidak** diubah. Gaji baru otomatis mengurangi sisa
  yang tampil di game.
- Baris kontrak berstatus 5 dengan klub = klubmu, tanggal mulai = tanggal game.
  Pemain yang sudah ada di daftar negosiasi barisnya dilengkapi di tempat;
  pemain lain mendapat baris baru di akhir tabel.

Saat menyimpan, hash SHA-512 save dihitung ulang dan file hasil didekripsi
ulang untuk verifikasi sebelum ditulis. Penyimpanan dibatalkan kalau ada byte
berubah di luar perubahan yang tercatat.

## Batasan

- Yang belum diketahui: indeks baris (w5) yang dipilih game untuk pemain baru,
  dan nilai pemain (w3). Editor memakai indeks berikutnya dan mengisi w3 dengan
  fee. Saat diuji dengan mengulang transfer Max Dowman di save `before`, hanya
  dua kolom itu yang berbeda dari yang ditulis game.
- **Penjualan pemain** menghapus baris kontrak dan menggeser baris berikutnya ke
  atas. Belum ada save contoh penjualan; cek di game.
- **Pinjaman tidak didukung.** Semua transfer bersifat permanen.
- **Free agent**: pemain "Lainnya (timnas, tanpa klub)" bisa berstatus *Others*
  atau *free agent*. Transfer free agent lewat editor diketahui bisa
  menghasilkan "pemain hantu".
- Riwayat fee di laporan keuangan game tidak ditambah.
- Klub AI tidak punya kontrak yang diubah; transfer AI ↔ AI hanya memindah skuad.
- Maksimal 40 pemain per klub. Nomor punggung dan lineup diatur di game.
- Offset budget PES 2021 vanilla belum diuji dengan save asli. Kalau strukturnya
  tidak cocok, edit budget otomatis dikunci.

## Rilis & update

Setiap push ke `main` menjalankan tes dan build Windows/macOS/Linux
(`.github/workflows/release.yml`). Kalau `version` di `Cargo.toml` belum punya
rilis `vX.Y.Z`, workflow langsung membuat rilis itu beserta file unduhannya.
Aplikasi mengecek rilis terbaru di repository `repository` pada `Cargo.toml`
saat dibuka; tombol **Cek update** mengecek manual.

Merilis versi baru = naikkan `version` di `Cargo.toml`, lalu push.

## Pengembangan

```bash
cargo run --release          # jalankan aplikasi
cargo run --release -- path/ke/ML00000000   # langsung buka save
cargo test --release         # tes (save ML sintetis, tanpa file game)
```

Kredit dan lisensi pihak ketiga: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
