# lms-grader — asisten penilaian LMS

Menarik kuis dan tugas dari LMS (dibuat untuk hendrikarisma.my.id, memakai API `/api/v1`-nya), lalu
**mengusulkan** nilai — termasuk esai dan laporan — memakai model lokal di Ollama pada mesin GPU sendiri.
Karya mahasiswa tidak dikirim ke layanan AI pihak ketiga. Status: **inisiasi (fase 1)**. Kerangka berjalan end-to-end; beberapa sinyal
belum tersambung dan dilaporkan apa adanya, tidak dipalsukan.

## Dua aturan dari dosen (struktural, bukan pengaturan)

1. **Usulan bukan nilai.** Grader hanya menulis usulan ke database lokalnya. Nilai baru dikirim ke LMS setelah
   dosen menyetujui di dashboard, dan mahasiswa tetap tidak melihatnya sampai dosen menekan **Rilis nilai** di
   LMS (`results_published`).
2. **Deteksi AI hanya tanda.** Skor kemungkinan-AI ditampilkan untuk dosen dan **tidak pernah** dikirim ke model
   penilai maupun mengubah nilai.

## Arsitektur

```
LMS /api/v1 ──sync──▶ Rust (lms-grader) ──▶ SQLite lokal data/grader.db
                         │                      items · proposals
                         ├─ analyzer.py (Python, stdin/stdout JSON)
                         │     ekstraksi teks · URL/DOI · embedding bge-m3 · [ONNX AI-detector]
                         └─ Ollama qwen3:8b ── usulan nilai 0–100 + alasan (JSON)
dashboard (web) ── dosen meninjau, setujui/ubah nilai → [fase 3] kirim nilai yang disetujui ke LMS
```

Rust: alur kerja, API, penyimpanan, dashboard. Python: hanya yang pustakanya jauh lebih baik di Python
(PDF/DOCX/ipynb, ONNX, matematika embedding).

## Menjalankan

```bash
git clone https://github.com/situkangsayur/lms-grader && cd lms-grader
cp .env.example .env          # isi LMS_TOKEN (token API staf, scope write, dari /admin/api)
python3 -m venv python/.venv && python/.venv/bin/pip install -r python/requirements.txt
cargo build --release
./target/release/lms-grader check     # ollama, analyzer, token
./target/release/lms-grader sync      # tarik kuis + tugas
./target/release/lms-grader grade 20  # buat usulan untuk 20 item
./target/release/lms-grader serve     # dashboard: http://127.0.0.1:8431 (default loopback)
```

Uji 10 Okt 2026 dengan satu jawaban esai contoh (bukan data mahasiswa): ±11 detik per item di GPU 8 GB; relevansi 0,685;
sumber (RFC 9293) terdeteksi; usulan nilai + alasan berbahasa Indonesia.

## Fase

| Fase | Isi | Status |
|---|---|---|
| 1 | Kerangka Rust + analyzer Python, sync via API, usulan nilai esai/teks dengan qwen3:8b, relevansi bge-m3, ekstraksi sumber | **selesai (inisiasi)** |
| 1b | Dashboard web: ringkasan, daftar dengan filter, detail per item (karya, kesesuaian, tanda AI, sumber, alasan), setujui/ubah nilai + catatan (lokal) | **selesai** |
| 2 | Baca **berkas** kiriman (PDF/DOCX/ipynb/md/zip), teks slide per pertemuan untuk **kesesuaian materi**, **detektor AI** ONNX (model VAR), rubrik per tugas dari deskripsi "Yang dikumpulkan/Penilaian", poin per soal esai, NIM mahasiswa | rencana |
| 3 | Dashboard tinjau: setujui/ubah per item atau per tugas, lalu **kirim** nilai + umpan balik lewat `POST /api/v1/submissions/{id}/grade` dan `/attempts/{id}/grade`; jalan terjadwal (systemd timer) | rencana |
| 4 | Kalibrasi: bandingkan usulan dengan nilai dosen pada sampel, laporan bias, ambang tanda AI | rencana |

## Yang perlu ditambah di LMS (untuk fase 2)

- `GET /api/v1/files/{id}` (staf) — unduh berkas kiriman; sekarang berkas hanya bisa diunduh lewat sesi browser admin.
- `GET /api/v1/quizzes/{id}` dengan soal dan opsi — supaya esai dinilai terhadap teks soalnya, bukan judul kuis.
- `GET /api/v1/assignments/{id}` lengkap dengan deskripsi — rubrik diambil dari "Yang dikumpulkan"/"Penilaian".

## Keterbatasan yang diketahui

- Model 8B bisa salah menilai; karena itu semua nilai melewati dosen.
- Detektor AI (fase 2) sering keliru pada tulisan Bahasa Indonesia dan pada mahasiswa yang menulis rapi — hanya tanda.
- Kuis pilihan ganda sudah dinilai otomatis oleh LMS; grader hanya menangani esai dan tugas.

## Dashboard

`lms-grader serve` menyajikan dashboard HTML (tanpa framework JS): **Ringkasan** (jumlah item, usulan menunggu,
tanda AI), **Usulan** (filter status/mata kuliah), dan **detail** per item. Keputusan dosen disimpan lokal
(`final_mark`, `note`, status `accepted`); pengiriman ke LMS adalah fase 3.

Akses jaringan: secara default hanya `127.0.0.1`. Untuk membukanya lewat jaringan privat (mis. WireGuard), isi
`GRADER_BIND` dengan alamat itu **dan** `GRADER_DASH_PASSWORD` — tanpa password, program menolak berjalan di alamat
non-loopback. Login memakai HTTP Basic, user `dosen`. Batasi juga di firewall hanya ke perangkat dosen.

Contoh service systemd (user):

```ini
[Service]
WorkingDirectory=%h/lms-grader
ExecStart=%h/lms-grader/target/release/lms-grader serve
Restart=on-failure
[Install]
WantedBy=default.target
```

## Keamanan dan privasi

- `.env` (token API) dan `data/` (salinan karya mahasiswa + usulan nilai) **tidak pernah** masuk repo — lihat `.gitignore`.
- Dashboard hanya mendengar di `127.0.0.1`; jangan dibuka ke jaringan.
- Token API LMS cukup untuk satu akun staf; cabut di `/admin/api` kalau mesin ini tidak dipakai lagi.

## Lisensi

Hak cipta © 2026 Hendri Karisma. Dirilis di bawah **GNU Affero General Public License v3.0 atau lebih baru**
(`AGPL-3.0-or-later`, lihat `LICENSE`). Anda boleh memakai, mengubah, dan menyebarkan ulang — tetapi versi yang
diubah, termasuk yang hanya dijalankan sebagai layanan lewat jaringan, wajib tetap terbuka dengan lisensi yang sama
dan menyertakan kode sumbernya.
