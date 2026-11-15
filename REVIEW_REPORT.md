# REVIEW REPORT — tự đánh giá bốn mẫu VTT

**Kết luận:** Cả bốn đầu ra parse được, giữ nguyên 272/272 cue và timecode; kiểm tra chuỗi từ điển nghiêm ngặt đạt 146/152 cue có thuật ngữ (96,05%). Stage 2 sửa 15 cue, không cue nào fallback. Chất lượng từ điển chưa đạt 100%; xem các vi phạm và đề xuất bên dưới.

## Môi trường và mẫu chạy

`hymt_bin/llama-server.exe` (llama.cpp `0.9.0-dev`, build 12182), chạy tuần tự trên CPU với 12 threads và `-ngl 0`. Model `Hy-MT2-30B-A3B-Q4_K_M.gguf` dùng Stage 1 (`-c 4096`, `q8_0`, port 8081); `Qwen3-30B-A3B-Q4_K_M.gguf` dùng Stage 2 (`-c 8192`, `q8_0`, port 8082). Cả hai model đã được nạp và endpoint health trả OK. Server Qwen đã dừng sau khi chạy; port 8082 đóng. RAM khả dụng thấp nhất đo được khi Qwen đang chạy là khoảng 0,58 GB, tăng lại 24,62 GB sau khi dừng.

Một file dịch cũ `in/S02E001.merged.zh.vi.vtt` đã tồn tại trước lượt này; nó được loại khỏi danh sách nguồn và không bị sửa. Các đầu vào dưới đây là những file tiếng Trung đầu tiên theo tên; hai file đầu trong `input/` đều thuộc tập 1.

| Nguồn | Cue | Đầu ra |
|---|---:|---|
| `in/S02E001.merged.zh.vtt` | 58 | [S02E001.merged.zh.vi.vtt](./output/review_test/in/S02E001.merged.zh.vi.vtt) |
| `in/S02E002.merged.zh.vtt` | 82 | [S02E002.merged.zh.vi.vtt](./output/review_test/in/S02E002.merged.zh.vi.vtt) |
| `input/第001集.merged.zh.vtt` | 71 | [第001集.merged.zh.vi.vtt](./output/review_test/input/第001集.merged.zh.vi.vtt) |
| `input/第001集.zh.vtt` | 61 | [第001集.zh.vi.vtt](./output/review_test/input/第001集.zh.vi.vtt) |

Tổng cộng 272 cue. Cả bốn checkpoint dùng cùng fingerprint từ điển `160f30f2f751152b1ed58eb58f304bb2916ac6680b0d51593d748569685bd9d4`.

## 1. Cấu trúc và timecode

Đã đọc lại cả bốn đầu ra bằng `vtt_core::VttDocument::parse`:

- Parse thành công **4/4**; số cue khớp **272/272**.
- Start/end timecode, identifier, settings và inline tag giữ nguyên chính xác giữa nguồn và bản dịch; không có sai lệch timecode.
- Có 32 chú thích `《...》` trong nguồn và 32 cặp `〈...〉` trong đầu ra. Không còn ký tự `《` hoặc `》` trong bản dịch; ngoặc đầu ra cân bằng.

## 2. Tuân thủ từ điển

Phép đo ở cấp cue: tìm các mục từ điển xuất hiện trong câu nguồn, chỉ tính mục dài nhất khi một khóa chứa khóa ngắn hơn, rồi kiểm tra chuỗi bản dịch có xuất hiện nguyên văn (không phân biệt hoa/thường) trong cue đích. Đây là kiểm tra chuỗi, không thay thế đánh giá ngữ nghĩa.

| Kết quả | Số cue |
|---|---:|
| Cue nguồn có ít nhất một mục từ điển | 152 |
| Tất cả mục được kiểm tra đều khớp | 146 |
| Có ít nhất một mục không khớp | 6 |
| Tỷ lệ khớp nghiêm ngặt | **96,05%** |

Các sai khác bị phép đo bắt được:

- `in/S02E001.merged.zh.vtt`, cue 14: `你们` → “hai người”, không có chuỗi `ngươi`.
- `input/第001集.merged.zh.vtt`, cue 4 và `input/第001集.zh.vtt`, cue 3: câu có `我`/`杀戮`; bản dịch lược đại từ và dùng “giết chóc/cuộc chiến” thay cho mục đích từ vựng “ta/sát lục”.
- Hai file `input/`, lần lượt cue 61 và 51: `修为` không xuất hiện thành chuỗi “tu vi” trong câu dịch “cả hai cùng tu luyện”.
- `input/第001集.zh.vtt`, cue 28: `你们` → “các người”, không có chuỗi `ngươi`.

Một số sai khác trên là diễn đạt tự nhiên hoặc lược chủ ngữ, nhưng chúng vẫn không đáp ứng yêu cầu tuân thủ chuỗi tuyệt đối. Các mục trọng điểm có mặt trong mẫu:

| Từ nguồn | Số lần có mặt | Khớp |
|---|---:|---:|
| `陆清安` → Lục Thanh An | 2 | 2/2 |
| `徐清莹` → Từ Thanh Oánh | 4 | 4/4 |
| `清莹` → Thanh Oánh | 6 | 6/6 |
| `淼淼` → Miểu Miểu | 2 | 2/2 |
| `识海` → thức hải | 2 | 2/2 |
| `传音` → truyền âm | 1 | 1/1 |
| `无垢帝位` → Vô Cấu Đế Vị | 2 | 2/2 |
| `陆子萱`, `陆淼淼`, `九狱弑仙剑`, `血戮战装`, `甜甜` | 0 | Không xuất hiện trong các mẫu |

Nguồn có biến thể `陆紫萱` một lần, không phải `陆子萱`; đầu ra vẫn là “Lục Tử Huyên”, nhưng biến thể này không được khóa từ điển hiện tại khớp trực tiếp. Các chuỗi Pinyin `Lu Zi Xuan`, `Qing Ying`, `Lu Miao Miao` không xuất hiện trong bốn đầu ra.

## 3. Nhất quán xưng hô và ngữ cảnh

Mỗi đoạn dưới đây gồm 8 cue liên tiếp; hai đoạn đầu cắt qua ranh giới batch Stage 2 20/21. Stage 2 nhận 5 cue trước làm ngữ cảnh.

**`S02E001.merged.zh.vtt`, cue 18–25 (qua batch 20/21):**

> 18. Hơi thở của tiểu Hắc Tử.<br>
> 19. A. · 20. A.<br>
> 21. Tiểu Hắc Tử, ngươi thật là chẳng biết nhớ lấy bài học gì cả.<br>
> 22. Lại đến rình mò nữa.<br>
> 23. 〈Hắc Long Thân Ngoại Hóa Thân〉<br>
> 24. Chủ nhân ơi, chủ nhân tốt của ta!<br>
> 25. Ta quyết định sẽ trở thành thú hợp đồng của ngài một lần nữa.

**`S02E002.merged.zh.vtt`, cue 18–25 (qua batch 20/21):**

> 18. 〈Các động tác nguy hiểm xin đừng bắt chước〉<br>
> 19. Ta nghe hắn bảo số lượng thần thông trong tay không dưới năm trăm loại.<br>
> 20. Từ Nhất Huyền cho đến Lục Huyền đều có cả.<br>
> 21. Thân phân thân ngoại hình của ta chính là một dạng trong Lục Huyền Thần Thông.<br>
> 22. Vậy thì càng phải bắt sống hắn mới được.<br>
> 23. Nhưng ngươi tốt nhất nên diễn một vở kịch cho hắn xem.<br>
> 24. Ta muốn tạo ra ảo ảnh về cái chết của hắn.<br>
> 25. Sau đó ngươi lại cầu xin tha mạng giúp hắn.

**`input/第001集.merged.zh.vtt`, cue 28–35 (mẫu vai vế gia đình):**

> 28. Không thể trì hoãn thêm một khắc nào nữa. · 29. Nếu không mọi công sức trước đó sẽ trở thành vô ích.<br>
> 30. Phu quân ơi. · 31. Ta và ngươi hãy hòa ly đi.<br>
> 32. Đừng làm phiền cha.<br>
> 33. Miểu Miểu không muốn các ngươi chia lìa.<br>
> 34. 〈Con gái cả – Lục Tử Huyên〉<br>
> 35. Ta mong mình có thể mạnh hơn nữa.

Các đoạn cho thấy `ta/ngươi`, `phu quân` và quan hệ cha/con được giữ ở phần lớn câu; tuy nhiên câu 32 (“Đừng làm phiền cha”) cần rà lại theo ngữ cảnh. Cặp `sư huynh–sư muội` không xuất hiện trong bốn nguồn này nên chưa thể kết luận về cặp xưng hô đó. Hai file tập 1 cũng dịch cùng câu `不要爹爹` khác nhau (“Đừng làm phiền cha” / “Đừng gọi cha nữa”), nên cần chuẩn hóa ở bước hậu kiểm.

## 4. Hiệu năng CPU và sửa đổi Stage 2

Token/giây là tốc độ sinh token có trọng số theo tổng token và thời gian sinh từ log llama-server; thời gian request gồm cả prompt eval và generation.

| Giai đoạn | Batch dự kiến | Request thực tế | Tổng thời gian server | Trung bình mỗi batch | Token sinh | Tốc độ sinh |
|---|---:|---:|---:|---:|---:|---:|
| Stage 1 — Hy-MT2 | 30 | 38 | 651,4 giây (10 phút 51 giây) | 21,7 giây/batch dự kiến | 6.348 | 11,48 token/giây |
| Stage 2 — Qwen3 | 16 | 16 | 3.014,9 giây (50 phút 15 giây) | 188,4 giây/batch; median 199,2 giây | 27.249 | 9,77 token/giây |

Stage 1 có 8 request nhiều hơn 30 batch danh nghĩa; log không đủ để phân biệt chính xác số retry và batch split. Stage 2 sửa **15/272 cue (5,51%)**, không có cue fallback:

| File | Stage 2 sửa | Fallback |
|---|---:|---:|
| `S02E001.merged.zh.vtt` | 1/58 | 0 |
| `S02E002.merged.zh.vtt` | 9/82 | 0 |
| `第001集.merged.zh.vtt` | 1/71 | 0 |
| `第001集.zh.vtt` | 4/61 | 0 |

## 5. Đề xuất trước khi chạy toàn bộ dữ liệu

1. Bổ sung khóa cụ thể `你们` để tránh xung đột với `你=ngươi`; thêm biến thể `陆紫萱` nếu đây là tên cần chuẩn hóa. Thêm bước kiểm tra cue có từ khóa nhưng thiếu bản dịch đích, rồi yêu cầu sửa hoặc đánh dấu hậu kiểm.
2. Rà các mục `杀戮` và `修为` bằng test phrase-level; prompt hiện tại chưa giữ được chuỗi đích trong mọi ngữ cảnh dù glossary đã có mục.
3. Qwen Stage 2 mất khoảng 50 phút cho 272 cue và sinh 27.249 token. Trước khi mở rộng, chạy thử A/B với `--reasoning off` (hoặc ngân sách reasoning thấp) trên cùng mẫu, giữ nguyên context và sampler, rồi so chất lượng và token/giây.
4. Bổ sung mẫu có hội thoại `师兄/师妹` vào bộ đánh giá; bốn file này không kiểm chứng được cách xưng hô đó.
5. Đã phát hiện và sửa bộ quét thư mục để bỏ qua file dịch có hậu tố `.vi.vtt`, tránh lấy đầu ra cũ làm nguồn khi chạy lại `--input in`.

## 6. A/B mới — tắt reasoning và glossary từ caption

Chạy tuần tự trên CPU với hai nguồn `in/S02E001.merged.zh.vtt` (58 cue) và `in/S02E002.merged.zh.vtt` (82 cue). Caption của cả hai tập được gửi trong một request Stage 1; cache chung được ghi tại `work/caption-ab/checkpoints/auto_glossary.json` với 8 mục. Các file nguồn gốc không bị sửa.

| Giai đoạn | Thời gian CLI | Request | Prompt token | Token sinh | Tốc độ sinh có trọng số |
|---|---:|---:|---:|---:|---:|
| Stage 1 — Hy-MT2, batch mặc định 20 | 359,8 giây (6 phút) | 9 | 2.559 | 2.884 | 10,35 token/giây |
| Stage 2 — Qwen3, reasoning budget 0, batch mặc định 35 | 640,3 giây (10 phút 40 giây) | 5 | 5.469 | 4.268 | 8,92 token/giây |

Stage 1 có một request caption-glossary và năm batch danh nghĩa; log ghi nhận chín request tổng cộng, nên có thêm request do retry/tách batch. Stage 2 có đúng năm request cho năm batch. Không có thẻ `<think>`/`</think>` hoặc các chuỗi Pinyin `Lu Zi Xuan`, `Qing Ying`, `Lu Miao Miao` trong hai đầu ra. Auto-glossary khớp **9/9** lần xuất hiện được kiểm tra; tên nhân vật có trong mẫu nhất quán.

Đã parse lại hai đầu ra bằng `vtt_core::VttDocument::parse`: **140/140 cue**, identifier, settings và timecode giữ nguyên; 25 cặp chú thích `《...》` được xuất thành 25 cặp `〈...〉`, không còn `《`/`》` trong đầu ra. Stage 2 sửa **6/140 cue (4,29%)**, không có cue fallback.

Hai server chạy tuần tự; khi Qwen đang nạp/chạy, RAM trống thấp nhất đo được khoảng 0,28 GB và đã phục hồi lên khoảng 27 GB sau khi dừng server.

Kiểm tra chuỗi nghiêm ngặt trên từ điển thủ công đạt **57/60 cue (95%)** có mục từ vựng; còn ba chỗ model diễn đạt lại hoặc lược từ:

- Tập 1, cue 10: `我` (“ta”) bị lược trong “Ngừng ngay đi!”.
- Tập 1, cue 14: `你们` (“các ngươi”) được diễn đạt thành “hai người”.
- Tập 1, cue 32: `你` (“ngươi”) không có chuỗi đích trong bản dịch.

Vì vậy, kiểm tra này xác nhận tắt reasoning và sự nhất quán của tên, nhưng **chưa đạt 100% tuân thủ chuỗi từ điển**. Thời gian Stage 2 đo được là 10 phút 40 giây cho 140 cue, không phải “vài phút”; quy đổi tuyến tính thô ra khoảng 20,7 phút/272 cue. So sánh với baseline 50 phút/272 cue chỉ mang tính định hướng vì mẫu và số batch khác nhau; kết quả này chưa chứng minh mức tăng tốc 15x. Đề xuất bổ sung hậu kiểm bắt buộc cho cue chứa từ điển: yêu cầu sửa nếu thiếu bản dịch nguyên văn, hoặc đánh dấu lỗi để người dùng rà soát thay vì âm thầm coi là đạt.

## 7. ERNIE-4.5-21B-A3B-Q4_K_M — tải model và trạng thái thực nghiệm

Đã tải bản `Q4_K_M` từ [unsloth/ERNIE-4.5-21B-A3B-PT-GGUF](https://huggingface.co/unsloth/ERNIE-4.5-21B-A3B-PT-GGUF), revision `de45a831ceac3c876bb63f39bc60406a7d07e3bb` (metadata ghi giấy phép Apache-2.0). Tệp nguồn `ERNIE-4.5-21B-A3B-PT-Q4_K_M.gguf` được lưu với tên `models/ERNIE-4.5-21B-A3B-Q4_K_M.gguf`.

| Kiểm tra tải model | Kết quả |
|---|---:|
| Kích thước tệp | 13.331.015.456 bytes (12,42 GiB) |
| SHA-256 | `eac44e96282fee37140f82f435d4c37c4ec0edd42fe5d93f09e6ed8786a6cff0` |
| Dung lượng trống ổ C: trước tải | 72.806.797.312 bytes |
| Dung lượng trống ổ C: sau tải | 59.472.236.544 bytes (55,39 GiB) |

**Inference thực tế chưa chạy được trong phiên này.** Lệnh khởi chạy server và gọi endpoint Stage 2 bị môi trường từ chối trước khi thực thi theo quy tắc `Domain`; đã xác nhận không có tiến trình `llama-server.exe`, listener cổng 8082, `master_glossary.json` hay đầu ra `output/ernie_test/` nào được tạo. Vì vậy không có dữ liệu thực nghiệm để báo cáo nội dung glossary do ERNIE sinh, token/giây, mức RAM, chất lượng dịch hay tỷ lệ cue Stage 2 sửa/fallback. Không dùng kết quả mock hoặc số liệu Qwen ở mục 6 để thay thế các số liệu này.

Hai caption đầu vào tồn tại và hai VTT nguồn có 58 và 82 cue. Để hoàn tất phép đo, cần chạy lại trong môi trường được phép bind/gọi endpoint local `127.0.0.1:8082`; khi đó chạy `build-glossary`, Stage 1 và Stage 2 tuần tự, rồi bổ sung kết quả đo và kiểm tra đầu ra vào mục này.

**Lưu ý về checkpoint:** không chạy Stage 1 hai lần cho hai file riêng lẻ với cùng `--work-dir work/ernie_test`; chế độ input file đơn dùng trực tiếp thư mục này và lần chạy thứ hai sẽ thay checkpoint của lần đầu. Hãy đặt đúng hai file vào một thư mục input riêng, rồi dùng `--input <thư_mục_đó>` ở cả Stage 1 và Stage 2. Chế độ thư mục tạo checkpoint riêng từng tập bên dưới `work/ernie_test/` và vẫn nạp glossary chung từ thư mục gốc.
