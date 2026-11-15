# Nhật ký thay đổi

## [Unreleased]

### Thêm
- Thêm workspace khung gồm `vtt-core`, `llm-client`, `translator-engine` và `vtt-cli`, giữ package `translate` hiện hữu.
- Thêm tài liệu phương pháp WebVTT, kiến trúc checkpoint và luồng pipeline hai tầng trong `docs/`.
- Triển khai `vtt-core::VttDocument` để parse/serialize lossless, trích speaker, giữ nguyên inline tags và xuống dòng cue.
