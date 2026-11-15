# REVIEW REPORT — kiểm thử thực tế bị chặn

**Trạng thái:** Chưa có kết quả dịch mới để đánh giá. Báo cáo này ghi lại tình trạng môi trường và các mẫu đã chọn; không khẳng định chất lượng bản dịch hoặc hiệu năng mô hình.

## Môi trường và mẫu dự kiến

`hymt_bin/llama-server.exe` có mặt (llama.cpp `0.9.0-dev`, build 12182). Thư mục `models/` hiện không chứa file nào; không có GGUF trong `hymt_bin/`. Vì vậy không thể khởi động Stage 1/2 hoặc tạo đầu ra cho lần chạy này. Server không được khởi chạy.

Theo thứ tự tên file, bốn mẫu được chọn là:

| Thư mục | File | Kích thước | Số dòng timing ước tính* |
|---|---|---:|---:|
| `in/` | `S02E001.merged.zh.vtt` | 3,803 B | 58 |
| `in/` | `S02E002.merged.zh.vtt` | 5,861 B | 82 |
| `input/` | `第001集.merged.zh.vtt` | 4,582 B | 71 |
| `input/` | `第001集.zh.vtt` | 3,425 B | 61 |

`plan.md` mô tả lỗi lịch sử ở các tập 1–4. Hai file đầu tiên theo tên trong `input/` đều thuộc tập 1; các file trên không được thay bằng mẫu khác.

\\* Đếm sơ bộ dòng khớp định dạng timestamp, không phải kết quả kiểm chứng bằng `vtt_core`.

## 1. Cấu trúc và timecode

**Chưa đánh giá đầu ra:** chưa có file `.vi.vtt` nào được sinh trong lần chạy này. Không thể xác nhận parse bằng `VttDocument`, số cue, timecode, hay ngoặc `〈...〉` ở bản dịch.

## 2. Tuân thủ từ điển

**Chưa đánh giá đầu ra:** chưa gửi request LLM. Không có tỷ lệ cue đạt/vi phạm và không kết luận về rò rỉ Pinyin hoặc dịch sai thuật ngữ.

## 3. Tính nhất quán xưng hô

**Chưa đánh giá đầu ra:** không có hội thoại đã dịch từ lần chạy này để trích dẫn hoặc kiểm tra qua ranh giới batch.

## 4. Hiệu năng CPU

**Chưa đo:** không có GGUF để nạp. Giây/batch, tokens/giây, tỷ lệ Stage 2 chỉnh sửa và số cue fallback đều chưa xác định.

## 5. Đề xuất và điều kiện chạy lại

1. Đặt GGUF Hy-MT2 và Qwen tương thích với server vào `models/`; tên file hiện có chưa được xác định nên chưa đưa ra tên model giả định.
2. Khi có model, chạy Stage 1 tuần tự trên bốn file đã chọn, rồi dừng server và chạy Stage 2 trên đúng checkpoint. Dùng cùng `--vocab vocabulary.txt` và cùng `--stage1-model` ở cả hai lượt để fingerprint checkpoint khớp.
3. Sau khi có đầu ra, chạy lại kiểm tra parse/cue/timecode/tag, rà các thuật ngữ đã lỗi trong `plan.md` (`陆子萱`, `清莹`, `陆淼淼`, `传音`, `识海`, `无垢帝位`, `九狱弑仙剑`, `血戮战装`, `甜甜`) và ghi metrics thực tế trước khi mở rộng sang toàn bộ thư mục.
4. Không dùng các file hiện có trong `output/` làm kết quả lần chạy này: chúng không khớp đầy đủ bốn mẫu được chọn và không được tạo bởi quy trình kiểm thử mới.
