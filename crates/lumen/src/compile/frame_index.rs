use super::CompiledOperation;

pub(super) fn build_frame_index(
    total_frames: u64,
    operations: &[CompiledOperation],
) -> Vec<Vec<usize>> {
    let mut frame_index = vec![Vec::new(); total_frames as usize];
    for (operation_index, operation) in operations.iter().enumerate() {
        if operation.is_mask {
            continue;
        }
        let start = operation.start_frame.min(total_frames);
        let end = operation.end_frame.min(total_frames);
        for frame in start..end {
            frame_index[frame as usize].push(operation_index);
        }
    }

    frame_index
}
