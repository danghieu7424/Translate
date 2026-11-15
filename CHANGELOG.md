# Nhật ký thay đổi

## [Unreleased]

### Thêm
- Thêm workspace khung gồm `vtt-core`, `llm-client`, `translator-engine` và `vtt-cli`, giữ package `translate` hiện hữu.
- Thêm tài liệu phương pháp WebVTT, kiến trúc checkpoint và luồng pipeline hai tầng trong `docs/`.
- Triển khai `vtt-core::VttDocument` để parse/serialize lossless, trích speaker, giữ nguyên inline tags và xuống dòng cue.
- Triển khai `llm-client::LlmClient` cho OpenAI-compatible chat completions, timeout CPU dài, sampler và response format JSON.
- Triển khai `translator-engine` với Stage 1/2 tuần tự, batching, retry/split có phân loại lỗi, context tối đa 5 cue, kiểm tra inline tags và fallback có cảnh báo.
- Thêm checkpoint JSON nguyên tử có SHA-256, xác thực model/config và resume cho pipeline dịch.
- Thêm API tiến độ theo batch/cue và thống kê số cue Stage 2 thay đổi hoặc fallback trong `translator-engine`.
- Triển khai `vtt-trans` với `--stage stage1|stage2|all`, progress bar trên stderr, output summary và hướng dẫn llama-server/Ollama.
- Thêm `Vocabulary::parse` và `--vocab`, lọc thuật ngữ theo SRC từng batch ở cả hai stage, kiểm tra fingerprint từ điển khi resume và chuẩn hóa `《...》` thành `〈...〉`.
- Mở rộng `vtt-trans` để quét `.vtt` cấp một trong thư mục, giữ checkpoint riêng theo file và xuất hàng loạt qua `--output-dir`.
