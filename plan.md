Dữ liệu thực tế từ các file kết quả vừa xuất (**`.vi_3.vtt`**) cho thấy việc tắt reasoning trace đã giúp tốc độ xử lý của Qwen 3.5 9B tăng lên đáng kể (8–12s/batch). Tuy nhiên, về mặt chất lượng dịch thuật, **Tầng 2 (Qwen 3.5 9B) đang gặp hiện tượng ảo giác (hallucination), tự ý sáng tạo bừa bãi và làm suy giảm nghiêm trọng độ chính xác của từ vựng chuẩn** so với Tầng 1.

Dưới đây là phân tích chi tiết các lỗi nặng phát sinh trong tập file `.vi_3.vtt`:

---

### 1. Phân Tích Các Lỗi Nặng Trong File `.vi_3.vtt`

#### A. Ảo giác tên riêng & Pinyin (Biến dạng từ vựng đã định nghĩa)

Dù `vocabulary.txt` đã cấu hình rõ tên nhân vật, Qwen 3.5 9B vẫn tự ý dịch sang Pinyin hoặc bịa tên mới:

* **Tập 1 - Line 34:** `〈Con gái lớn - Lu Zi Xuan〉` $\rightarrow$ Bị văng Pinyin (đúng phải là *Lục Tử Huyên*).


* **Tập 1 - Line 38:** `Qing Ying không thể lãng phí...` $\rightarrow$ Bị văng Pinyin (đúng phải là *Từ Thanh Oánh*).


* **Tập 3 - Line 1:** `〈Thanh Minh〉, đừng nghĩ nữa` $\rightarrow$ Bịa tên *Thanh Minh* cho *Từ Thanh Oánh* (`清莹`).


* **Tập 4 - Line 42-52:** `Mèo Mèo, phụ thân tặng con...` $\rightarrow$ Dịch tên *Lục Miểu Miểu* (`陆淼淼`) thành *Mèo Mèo*.


* **Tập 4 - Line 9:** `Cửu Yụt Diệt Tiên Kiếm` $\rightarrow$ Sai chính tả thô bạo (*Cửu Ngục Sát Tiên Kiếm*).


* **Tập 4 - Line 45:** `Bộ Chiến Trang Huyết Lỗ` $\rightarrow$ Dịch nhầm *Huyết Lục Chiến Trang* thành *Huyết Lỗ*.


* **Tập 4 - Line 68:** `Tất nhiên phải gọi là Tây Tây Nha` $\rightarrow$ Dịch `甜甜` (*Điềm Điềm*) thành *Tây Tây Nha*.



#### B. Hiện đại hóa thuật ngữ tiên hiệp (Ngô nghê)

* **Tập 2 - Line 80:** `Sau này liên lạc qua tin nhắn` $\rightarrow$ Từ gốc `传音` (*truyền âm*) bị Qwen 3.5 dịch thành **"tin nhắn"** (sử dụng từ ngữ điện thoại hiện đại trong phim cổ trang).


* **Tập 1 - Line 44:** `cưỡng lại và vào biển nhớ của ta` $\rightarrow$ Từ gốc `识海` (*thức hải*) bị dịch thành **"biển nhớ"**.


* **Tập 2 - Line 5:** `Để vào trong biển tri thức của ngươi...` $\rightarrow$ `识海` (*thức hải*) bị dịch thành **"biển tri thức"**.


* **Tập 2 - Line 25:** `sắp tới các ngươi chắc chắn sẽ trở thành đế đô` $\rightarrow$ Từ gốc `称帝` (*xưng đế*) bị dịch thành **"đế đô"** (thành phố thủ đô).


* **Tập 2 - Line 10 & 107:** `Vô Thù Đế` / `Vô Độ Thần` $\rightarrow$ Dịch sai hoàn toàn danh hiệu `无垢帝位` (*Vô Cấu Đế Vị*).


* **Tập 2 - Line 85:** `Nấu trong Tiên Khư nhiều năm...` $\rightarrow$ Từ gốc `藏在仙墟` (*ẩn nấp ở Tiên Khư*) bị gõ sai thành **"Nấu"**.


