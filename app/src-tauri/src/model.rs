use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct GroupSummary {
    pub id: u64,
    pub sim: u8,
    pub file_count: usize,
    pub total_bytes: u64,
    pub covers: Vec<String>,
    pub names: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct FrameRef {
    pub url: String,
    pub at: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: u32,
    pub duration: f32,
    pub size: u64,
    pub keep: bool,
    pub frames: Vec<FrameRef>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Evidence {
    pub matched_frames: usize,
    pub total_frames: usize,
    pub longest_run: usize,
    pub avg_hamming: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct GroupDetail {
    pub group: GroupSummary,
    pub files: Vec<FileEntry>,
    pub evidence: Evidence,
}

#[derive(Serialize, Clone, Debug)]
pub struct ScanProgress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ScanSummary {
    pub groups: usize,
    pub files: usize,
    pub matched_files: usize,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct Item {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub detail: String,
    pub kind: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ToolResult {
    pub tool: String,
    pub items: Vec<Item>,
    pub groups: Vec<GroupSummary>,
    pub total_files: usize,
    pub elapsed_ms: u64,
}
