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
- **`TranslationState`**: phiên bản schema, fingerprint đầu vào, cấu hình hai stage, trạng thái từng cue (`pending`, `drafted`, `refined`), bản nháp/bản cuối và tiến độ theo stage.
- **`CuePatch`**: `{ id, final_text }` cùng thông tin nguồn cần kiểm tra. Chỉ payload của cue tương ứng được phép thay đổi; patch trùng ID, ID không tồn tại hoặc vi phạm bảo toàn tag là lỗi.

## Checkpoint / resume

Checkpoint mặc định là JSON trong work directory. Nó chứa schema version, SHA-256 của đầu vào gốc, danh sách cue theo ordinal, kết quả đã hoàn tất và nhận diện/cấu hình model của từng stage. Không lưu lại hoặc sửa nội dung nguồn khi tiếp tục.

- Ghi sau mỗi batch hoàn tất; cập nhật bằng ghi file tạm trong cùng thư mục rồi thay thế checkpoint để tránh trạng thái dở dang.
- Khi resume, xác minh hash nguồn, số lượng/thứ tự cue, schema và cấu hình liên quan. Mismatch phải báo lỗi rõ ràng; không tự tiếp tục trên dữ liệu không tương thích.
- Chỉ đánh dấu batch hoàn tất sau khi kết quả đã được kiểm tra và checkpoint ghi thành công. Batch đang chạy khi bị ngắt được chạy lại, không được đánh dấu thành công trước.
- Stage 2 chỉ chạy từ checkpoint có draft đầy đủ. Cue chưa được sửa ở Stage 2 nhận `final_text = draft_text`; kết quả đó cũng được ghi checkpoint trước khi hoàn tất.
- Không ghi đè file đầu vào khi xuất. Lỗi đọc/ghi/serialize được trả về cho CLI; không giả lập thành công hoặc bỏ qua cue lỗi.

## Quản lý bộ nhớ và thứ tự chạy

Mặc định pipeline tuần tự: hoàn thành Stage 1 cho toàn file, lưu checkpoint, kết thúc/giải phóng model Stage 1, rồi mới khởi tạo model Stage 2. CLI stage riêng cho phép người dùng đổi model/server giữa hai bước. Không yêu cầu hai model cùng tồn tại trong RAM. `run` chỉ gọi hai stage tuần tự; không tự quyết định rằng cấu hình RAM đủ an toàn. Quản lý vòng đời tiến trình model thuộc cấu hình backend, không để engine giữ model nặng ngoài thời gian cần thiết.

## Ranh giới lỗi

Lỗi transport, timeout, JSON sai, ID ngoài batch, thiếu đầu ra Stage 1 hoặc patch không an toàn phải được thể hiện rõ. Retry có giới hạn; chia batch là fallback, không phải bỏ cue. Nếu batch đơn lẻ vẫn lỗi sau fallback, dừng với lỗi có stage và ID liên quan, giữ checkpoint các batch đã hoàn tất để resume.
