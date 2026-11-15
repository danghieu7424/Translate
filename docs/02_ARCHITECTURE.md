# 02 — Kiến trúc và trạng thái

## Workspace

Workspace Cargo gồm các crate mới `vtt-core`, `llm-client`, `translator-engine`, `vtt-cli`, đồng thời giữ package `translate` hiện hữu làm workspace member để không làm mất API và ví dụ đang có. Các crate mới được triển khai theo thứ tự phụ thuộc: parser trước, client sau, engine tiếp theo, CLI cuối cùng.

```text
vtt-cli -> translator-engine -> { vtt-core, llm-client }
```

`vtt-core` không phụ thuộc client/model. `llm-client` chỉ phụ trách transport và xác thực phản hồi giao thức. `translator-engine` quản lý prompt, batching, retry/split, patch và checkpoint. `vtt-cli` chịu trách nhiệm cấu hình, điều phối stage và báo tiến độ.

## Kiểu dữ liệu cốt lõi

- **`VttDocument`**: chuỗi các phần tử theo thứ tự nguồn, cùng metadata cần để round-trip (ví dụ loại newline). Chứa header, khối opaque và cue; không chuẩn hóa dữ liệu chưa sửa.
- **`VttCue`**: cue ID pipeline ổn định, cue identifier gốc tùy chọn, vùng timestamp/settings nguyên gốc, payload/tag và vị trí nguồn; `speaker: Option<String>` được trích từ `<v Name>`. Trạng thái dịch gắn với cue hoặc lưu riêng trong `TranslationState`, không ghi đè nguồn.
- **`TranslationState`**: phiên bản schema, SHA-256 đầu vào, fingerprint hai stage và trạng thái theo cue (`pending`, `drafted`, `refined`), gồm bản nháp/bản cuối.
- **`CuePatch`**: `{ id, final_text }`. Chỉ payload cue tương ứng được phép thay đổi; edit trùng ID, ID ngoài batch hoặc vi phạm bảo toàn tag được ghi cảnh báo, cue đó dùng lại draft Stage 1 và engine tiếp tục.

## Checkpoint / resume

Checkpoint mặc định là JSON trong work directory, kèm bản sao bất biến `source.vtt` để Stage 2 có thể dựng lại cấu trúc khi chỉ nhận `work_dir`. Checkpoint chứa schema version, SHA-256 của đầu vào gốc, danh sách cue theo ordinal, kết quả đã hoàn tất và nhận diện/cấu hình model của từng stage.

- Ghi sau mỗi batch hoàn tất; cập nhật bằng ghi file tạm trong cùng thư mục rồi thay thế checkpoint để tránh trạng thái dở dang.
- Khi resume, xác minh hash nguồn, số lượng/thứ tự cue, schema và cấu hình liên quan. Mismatch phải báo lỗi rõ ràng; không tự tiếp tục trên dữ liệu không tương thích.
- Chỉ đánh dấu batch hoàn tất sau khi kết quả đã được kiểm tra và checkpoint ghi thành công. Batch đang chạy khi bị ngắt được chạy lại, không được đánh dấu thành công trước.
- Stage 2 chỉ bắt đầu sau khi toàn bộ Stage 1 đã hoàn tất và checkpoint draft được ghi. Mỗi batch Stage 2 hoàn tất được checkpoint riêng; cue không có edit hợp lệ nhận `final_text = draft_text`.
- Không ghi đè file đầu vào khi xuất. Lỗi đọc/ghi/serialize được trả về cho CLI; không giả lập thành công hoặc bỏ qua cue lỗi.

## Quản lý bộ nhớ và thứ tự chạy

Mặc định pipeline tuần tự: hoàn thành Stage 1 cho toàn file, lưu checkpoint, rồi mới gửi request Stage 2. CLI stage riêng cho phép người dùng đổi model/server giữa hai bước. `run` không gửi hai stage song song. Việc giải phóng model nằm ở server/backend; engine chỉ giữ client HTTP nhẹ.

## Ranh giới lỗi

Lỗi transport, timeout, JSON sai hoặc thiếu đầu ra được retry có giới hạn rồi chia batch; nếu một cue vẫn lỗi, dừng với lỗi stage/ID và giữ checkpoint đã hoàn tất. HTTP 429 và lỗi server (5xx) có thể retry/split; lỗi cấu hình/request và HTTP 4xx khác dừng ngay, không tạo thêm request chia batch. Stage 2 bỏ qua edit có ID ngoài batch hoặc tag không khớp, ghi cảnh báo và giữ draft của cue tương ứng thay vì làm crash cả batch.
