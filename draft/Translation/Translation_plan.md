# Translation Architecture Plan (Bước 6 Pipeline)

## 1. Tổng quan
Dịch toàn bộ phụ đề tiếng Anh đã căn chỉnh mốc thời gian (`aligned.vtt`) sang phụ đề tiếng Việt (`vi.vtt`) phục vụ quá trình lồng tiếng (TTS Phase 2).

---

## 2. Kiến trúc 2 tầng (Hybrid Translation Engine)

```text
┌──────────────────────────────────────────────────────────────┐
│  Aligned English Subtitles (aligned.vtt / SubCue Timestamps) │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│  [Tầng 1] Google Translate RPC (translate_rpc.rs)            │
│  - Tốc độ cực nhanh, Zero Cost, không cần API Key            │
│  - Cơ chế Batch Chunking (25 câu/request)                    │
│  - Tự động fallback dịch câu đơn nếu JSON mảng bị lệch        │
│  - Đạt 95% độ chính xác về cấu trúc ngữ pháp tổng quát       │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│  [Tầng 2] Gemini Flash Term Corrector (term_corrector.rs)    │
│  - Tự động nạp GEMINI_API_KEY từ file .env                   │
│  - Chỉ sửa thuật ngữ chuyên ngành (music production, DAWs)   │
│  - Giữ nguyên beat, drill, producer, modules, 808s...        │
│  - Smart Fallback Chain:                                     │
│      gemini-3.5-flash → gemini-2.0-flash → gemini-1.5-flash  │
│  - Fallback an toàn: Nếu LLM lỗi → dùng bản dịch Tầng 1       │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│  Phụ đề Tiếng Việt Chuẩn WebVTT (vi.vtt / <video>.vi.vtt)    │
│  - Bảo toàn 100% mốc thời gian của aligned.vtt               │
│  - Tối ưu hóa nhịp thở tự nhiên cho bộ đọc TTS tiếng Việt    │
└──────────────────────────────────────────────────────────────┘
```

---

## 3. Đặc tả Kỹ thuật Chi tiết

### Tầng 1: Google Translate RPC (`translate_rpc.rs`)
- **Endpoint:** `https://translate.googleapis.com/translate_a/single`
- **Phương thức:** POST form-encoded với `client=gtx`, `sl=en`, `tl=vi`.
- **Cơ chế chống lỗi:**
  - `sanitize_text`: Làm sạch các ký tự nháy kép thông minh (`“`, `”`, `«`, `»`) tránh vỡ cấu trúc chuỗi JSON.
  - `chunk_size = 25`: Chia nhỏ danh sách câu để tránh quá tải kích thước URL/payload.
  - **Per-sentence Fallback:** Nếu mảng JSON trả về bị gộp câu hoặc không parse được, tự động chuyển sang dịch tuần tự từng câu đơn lẻ, triệt tiêu 100% nguy cơ crash pipeline.

### Tầng 2: Gemini Flash Term Corrector (`term_corrector.rs`)
- **Mục tiêu:** Giải quyết nhược điểm "dịch máy thô (literal translation)" của Google Translate đối với các thuật ngữ chuyên môn (ví dụ: *"cargo build"* $\rightarrow$ *"bản dựng hàng hóa"*, *"deep house"* $\rightarrow$ *"nhà sâu"*, *"put the reps in"* $\rightarrow$ *"đưa đại diện vào"*).
- **Batching & Rate Limit Defense:**
  - Chia nhỏ 30 câu/batch (`BATCH_SIZE = 30`).
  - Delay `BATCH_DELAY_MS = 4500ms` giữa các batch để tuân thủ ngưỡng Free-Tier Rate Limit của Google Generative AI API.
- **Multi-Model Fallback Chain:**
  1. Thử `gemini-3.5-flash` (nhanh, chất lượng cao).
  2. Nếu gặp HTTP 503 / 429 sau 2 lần retry $\rightarrow$ tự động chuyển sang `gemini-2.0-flash`.
  3. Nếu tiếp tục lỗi $\rightarrow$ chuyển sang `gemini-1.5-flash`.
  4. Nếu toàn bộ model fail $\rightarrow$ giữ nguyên bản dịch Google Tầng 1, đảm bảo pipeline liên tục và không gián đoạn.

---

## 4. Định dạng Output & Vị trí Lưu trữ
- `<tmp_dir>/subtitles/<video_name>.vi.vtt`: File phụ đề tiếng Việt chính thức.
- `<tmp_dir>/subtitles/vi.vtt`: File alias chuẩn hóa để các module downstream (TTS, Lip-Sync, Muxing) dễ dàng truy xuất.