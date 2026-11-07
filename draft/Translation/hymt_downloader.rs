/****
 * [ATOM]: hymt_downloader
 * Chức năng: Kiểm tra và tự động tải mô hình Tencent Hy-MT2 (GGUF 1.18B)
 * từ Hugging Face Hub nếu môi trường máy trắng / thiếu weights.
 ****/
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub const HYMT2_HF_REPO: &str = "tencent/Hy-MT2-1.8B-GGUF";
pub const HYMT2_GGUF_FILE: &str = "Hy-MT2-1.8B-Q4_K_M.gguf";

pub fn ensure_hymt2_model(models_dir: &Path) -> Result<PathBuf> {
    let target_dir = models_dir.join("hymt2");
    fs::create_dir_all(&target_dir)?;

    let gguf_dest = target_dir.join(HYMT2_GGUF_FILE);

    // 1. Kiểm tra nếu file đã tồn tại và đủ kích thước (> 500 MB)
    if gguf_dest.exists() {
        if let Ok(meta) = gguf_dest.metadata() {
            if meta.len() > 500 * 1024 * 1024 {
                return Ok(gguf_dest);
            }
        }
    }

    // 2. Kiểm tra các vị trí dự phòng trong scratch hoặc exe adjacent
    let scratch_cand = Path::new("scratch/hy_mt2_test/models").join(HYMT2_GGUF_FILE);
    if scratch_cand.exists() {
        if let Ok(meta) = scratch_cand.metadata() {
            if meta.len() > 500 * 1024 * 1024 {
                let _ = fs::copy(&scratch_cand, &gguf_dest);
                if gguf_dest.exists() {
                    return Ok(gguf_dest);
                }
                return Ok(scratch_cand);
            }
        }
    }

    // 3. Tải an toàn qua Shared Model Downloader (Tắt progress bar raw stderr để không vẽ đè lên TUI)
    crate::shared::tui_events::emit_log(format!(
        "[Translation/HyMT2] {} Đang tải mô hình GGUF {} từ {}...",
        cli_gui_form::Icons::NODE_LINK,
        HYMT2_GGUF_FILE,
        HYMT2_HF_REPO
    ));

    crate::shared::model_downloader::ensure_model_file(
        HYMT2_HF_REPO,
        HYMT2_GGUF_FILE,
        &gguf_dest,
    )?;

    crate::shared::tui_events::emit_log(format!(
        "[Translation/HyMT2] {} Mô hình Hy-MT2 GGUF đã sẵn sàng!",
        cli_gui_form::Icons::SUCCESS
    ));

    Ok(gguf_dest)
}
