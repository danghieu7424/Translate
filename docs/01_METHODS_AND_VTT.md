# 01 — Phương pháp xử lý WebVTT

## Mục tiêu và cam kết bảo toàn

`vtt-core` đọc WebVTT thành các vùng cấu trúc có thể nhận diện, nhưng giữ nguyên văn bản nguồn làm cơ sở để tái xuất. Khi chưa dịch một cue, `parse` rồi `serialize` phải cho lại nội dung giống hệt đầu vào, bao gồm thứ tự, khoảng trắng, dòng trống, kiểu xuống dòng, timestamp, settings và các khối không phải cue. Khi dịch, chỉ phần payload lời thoại của cue được phép thay đổi; mọi phần còn lại lấy nguyên từ nguồn.

Không chuẩn hóa timestamp, metadata, dấu câu, thẻ hoặc xuống dòng một cách ngầm định. Nếu đầu vào không thể phân tích an toàn, trả lỗi có vị trí/ngữ cảnh thay vì âm thầm bỏ nội dung.

## Các thành phần WebVTT cần nhận diện

1. **Header:** dòng `WEBVTT` bắt buộc, có thể có BOM UTF-8 và phần mô tả trên cùng dòng. Các dòng metadata như `Kind:` và `Language:` được giữ nguyên.
2. **Khối không phải cue:** `NOTE` (có thể nhiều dòng), `STYLE` và `REGION` được giữ nguyên như các khối độc lập. Không gửi các khối này tới mô hình dịch.
3. **Cue:** có thể có cue identifier trên dòng riêng, tiếp theo là timestamp `start --> end` và cue settings như `align:start position:0%`. Giữ nguyên identifier, timestamp và settings dạng văn bản gốc; parser không dựng lại timestamp từ số dấu phẩy động.
4. **Payload:** một hoặc nhiều dòng lời thoại, gồm văn bản và inline tags như `<v Speaker>`, `</v>`, `<i>`, `<b>`, `<c.color>`, `<ruby>`, `<rt>`, `<br>` và các timestamp tag. Giữ nguyên tag và vị trí của chúng; không dịch tên speaker, lớp CSS hoặc thuộc tính tag.
5. **Phân cách:** giữ nguyên các dòng trống và loại newline (`LF` hoặc `CRLF`) để phép round-trip không làm thay đổi bố cục.

## Mô hình phân tích và dịch

- Tài liệu là chuỗi các đoạn theo thứ tự nguồn: header, khối opaque, cue và phần phân cách. Cách biểu diễn này tránh làm mất cấu trúc chưa được hiểu đầy đủ.
- Mỗi `VttCue` giữ riêng identifier WebVTT (nếu có), timestamp/settings gốc, payload gốc, speaker tùy chọn và vị trí nguồn. Cue ID dùng trong pipeline là **ordinal 1-based của cue trong tài liệu**, độc lập với cue identifier của WebVTT; ID này ổn định giữa hai tầng và checkpoint.
- `VttCue` giữ nguyên payload như văn bản, đồng thời trích chuỗi inline tag theo thứ tự và `speaker = Some(Name)` từ `<v Name>`. `set_payload` chỉ nhận nội dung có đúng chuỗi tag gốc; khi xuất, tag được giữ nguyên văn và đúng thứ tự.
- Với cue nhiều dòng, prompt biểu diễn nội dung của một cue như một bản ghi có ID duy nhất; parser phản hồi nhận các dòng tiếp nối cho tới ID kế tiếp. Khi xuất bản dịch mới, giữ line break do phản hồi cung cấp; nếu phản hồi là một dòng, cho phép reflow có kiểm soát thay vì thay đổi timestamp hay cấu trúc cue. Nội dung chưa dịch luôn giữ line break gốc.
- Trước khi ghép lại, kiểm tra tag/placeholder được bảo toàn và cue patch chỉ nhắm tới cue ID hợp lệ. Không để nội dung mô hình sửa header, timestamp, settings hay ranh giới cue.

## Lỗi và giới hạn

Parser phải phân biệt cấu trúc không hợp lệ với khối opaque hợp lệ. Lỗi cấu trúc cần chỉ ra vị trí và không được trả tài liệu một phần dưới dạng thành công. Round-trip test bao gồm header metadata, `NOTE`, cue identifier, cue settings, cue nhiều dòng, speaker tag và inline formatting. Bảo toàn byte tuyệt đối áp dụng cho tài liệu chưa sửa; với cue được dịch qua `set_payload`, bảo toàn cấu trúc, chuỗi tag và kiểu xuống dòng của cue, không áp dụng cho payload văn bản đã thay đổi.
