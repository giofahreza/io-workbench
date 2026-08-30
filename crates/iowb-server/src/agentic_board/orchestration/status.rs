fn task_priority_rank(priority: &str) -> u8 {
    match normalize_priority(Some(priority)) {
        TASK_PRIORITY_P0 => 0,
        TASK_PRIORITY_P1 => 1,
        TASK_PRIORITY_P2 => 2,
        TASK_PRIORITY_P3 => 3,
        _ => 3,
    }
}
