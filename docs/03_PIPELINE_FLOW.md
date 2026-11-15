# 03 — Luồng hai tầng, prompt và giao thức

## Cấu hình dùng chung

Stage 1 dùng model dịch Hy-MT2-30B-A3B GGUF Q4_K_M với batch mặc định 10 cue. Stage 2 dùng Qwen-30B-A3B GGUF Q4_K_M với batch mặc định 20 cue. Endpoint và tên model được cấu hình riêng cho mỗi stage để hỗ trợ server độc lập hoặc server có cơ chế đổi model. Tên model trong tài liệu là mục tiêu cấu hình; client không giả định model đã cài hoặc server đã sẵn sàng.

Tham số sampler mặc định: `temperature = 0.15`, `top_p = 0.9`, `repeat_penalty = 1.15`, `frequency_penalty = 0.2`. Client gửi cả bốn trường ở cấp request; `repeat_penalty` là tham số backend-specific và server có thể từ chối hoặc bỏ qua trường không hỗ trợ. `base_url` chấp nhận origin, `/v1` hoặc endpoint `/v1/chat/completions`. Timeout request mặc định 600 giây và không cho đặt thấp hơn 300 giây; connect timeout mặc định 15 giây.

## Stage 1 — Draft

1. Lấy các cue theo thứ tự và chia thành nhóm tối đa 10; ID là ordinal 1-based của cue trong tài liệu.
2. Gửi bản ghi `[ID] <nội dung nguồn>`; giữ thông tin speaker và inline tags theo chính sách bảo vệ của `vtt-core`.
3. System prompt yêu cầu dịch sát nghĩa sang tiếng Việt, giữ nguyên tag/placeholder, không thêm lời dẫn, và trả đúng một bản ghi cho mỗi ID. Các dòng tiếp nối thuộc về ID gần nhất; ID là ranh giới bản ghi.
4. Kiểm tra tập ID đầu ra phải khớp chính xác với tập ID đầu vào (không thiếu, trùng hoặc thừa), nội dung không rỗng nếu nguồn có lời thoại, và tag được bảo toàn. Chỉ sau kiểm tra mới đặt `draft_text` và checkpoint batch.
5. Nếu phản hồi sai định dạng hoặc ID, thử lại cùng batch tối đa 3 lần sau lần gọi đầu (tối đa 4 lần gọi). Sau đó chia batch lỗi làm hai và xử lý tuần tự; nếu một cue vẫn lỗi thì dừng, báo ID và giữ checkpoint đã hoàn tất.

## Stage 2 — Context-aware refinement

Chỉ bắt đầu sau khi Stage 1 đã hoàn tất toàn bộ file và lưu checkpoint. Chia cue có draft thành batch tối đa 20. Trước mỗi batch, lấy tối đa 5 cue liền trước đã có `final_text` thành công; context chỉ đọc và không thuộc tập cue có thể sửa. Các cue đầu file có ít context hơn; batch đầu không có context. Context dùng bản cuối đã hiệu đính, không dùng draft của batch trước nếu chưa hoàn tất Stage 2.

Đầu vào batch có dạng:

```text
[CONTEXT - READ ONLY - DO NOT TRANSLATE]
[id] (Speaker: <speaker nếu có>) <final_text>

[CUES TO REVIEW]
[id] (Speaker: <speaker nếu có>) SRC: <nguồn> | DRAFT: <bản nháp>
```

System prompt yêu cầu so sánh nguồn và draft, sửa nghĩa hoặc xưng hô khi cần, nhất quán với context và speaker; không dịch context, không thay đổi tag/placeholder, không thêm lời giải thích. Trả về duy nhất JSON diff:

```json
{
  "edits": [
    {"id": 102, "final_text": "Mày đang làm cái quái gì thế?"},
    {"id": 105, "final_text": "Tao đã bảo là không được lại gần rồi mà!"}
  ]
}
```

JSON Schema yêu cầu object có `edits` là mảng; mỗi phần tử có `id` integer và `final_text` string không rỗng, không cho phép thuộc tính không khai báo. Edit trùng ID hoặc ID ngoài batch được ghi cảnh báo và bỏ qua; cue không có edit hợp lệ hoặc có tag không khớp giữ draft Stage 1 làm `final_text`. Vì đây là diff thưa, thiếu ID trong `edits` là hợp lệ.

## HTTP, response format và kiểm tra

Client gửi OpenAI-compatible `POST /v1/chat/completions` qua `reqwest`, hỗ trợ `response_format: {"type":"json_object"}` và `response_format: {"type":"json_schema","json_schema":{"name":...,"strict":true,"schema":...}}`. Client truyền schema đúng nguyên dạng; server phải hỗ trợ response format tương ứng để ràng buộc đầu ra. Engine vẫn phải parse và validate nội dung JSON cục bộ vì server có thể không thực thi hoặc không hỗ trợ schema.

Client không retry ngầm: lỗi transport/timeout, HTTP status, JSON response không hợp lệ hoặc thiếu content được trả rõ cho caller. Engine chịu trách nhiệm thử lại tối đa 3 lần sau lần gọi đầu khi lỗi phù hợp, kiểm tra đầu ra theo batch rồi chia đôi batch lỗi; lỗi cấu hình/request và HTTP 4xx (ngoại trừ 429) dừng ngay mà không retry/split. Khi chỉ còn một cue mà vẫn lỗi retryable, engine trả lỗi rõ ràng. Đối với Stage 2, chỉ lỗi JSON/schema/ID ngoài batch là invalid; edits rỗng hoặc thiếu ID là kết quả hợp lệ. Retry/split không được làm mất cue hoặc áp dụng một phần patch chưa validate.

## Hoàn tất và tiếp tục

Mỗi batch chỉ được commit vào `TranslationState` sau khi cả phản hồi và patch vượt kiểm tra. Stage 2 xây context từ các cue đã commit trước đó theo thứ tự tài liệu. Resume bỏ qua batch đã checkpoint thành công và tiếp tục từ trạng thái cue; xuất cuối dùng `final_text` nếu có, nếu không dùng draft, ghép vào cấu trúc VTT gốc và giữ nguyên mọi vùng bất biến.

CLI `vtt-trans` dùng `--stage stage1|stage2|all` (mặc định `all`). Chạy riêng Stage 1 ghi checkpoint và bản nguồn vào work directory; Stage 2 đọc lại các file này để có thể khởi động server/model khác trước khi tiếp tục. Thanh tiến trình dựa trên số cue hoàn tất và ETA trung bình, còn batch count được cập nhật khi engine phải chia nhỏ batch. Hướng dẫn lệnh server nằm trong `crates/vtt-cli/README.md`.
