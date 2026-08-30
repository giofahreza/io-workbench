async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
        service: PRODUCT_NAME.to_string(),
        version: VERSION.to_string(),
        config_dir: state.config.config_dir.display().to_string(),
        database_path: state.config.database_path.display().to_string(),
        server_time: Utc::now(),
    })
}

async fn server_status(State(state): State<AppState>) -> Json<ServerStatusResponse> {
    Json(state.config.server_status(VERSION))
}

async fn runtime_metrics(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "metrics": runtime_metrics_payload(&state).await?,
    })))
}

async fn runtime_metrics_payload(state: &AppState) -> Result<Value> {
    // Metrics only needs the project count. Hydrating every project's external
    // sessions here made a lightweight mobile connection perform a history
    // synchronization before the UI requested any chat list.
    let projects = state.storage.list_projects()?;
    let active_sessions = state.sessions.list_active().await;
    let processes = state.processes.list().await;
    let resources = system_resource_metrics(&state.config.workspace_root).await;
    let process_uptime_seconds = resources
        .get("processUptimeSeconds")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(serde_json::json!({
        "timestamp": Utc::now(),
        "memory": process_memory_metrics().await,
        "resources": resources,
        "server": {
            "status": "ok",
            "appRoot": state.config.workspace_root.display().to_string(),
            "installMode": "rust",
            "packageName": PRODUCT_NAME,
            "version": VERSION,
            "uptimeSeconds": process_uptime_seconds,
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "pid": std::process::id(),
            "port": state.config.port.to_string(),
            "environment": env::var("IO_WORKBENCH_ENV").unwrap_or_else(|_| "local".to_string()),
        },
        "projects": {
            "count": projects.len()
        },
        "sessions": {
            "active": active_sessions.len()
        },
        "processes": {
            "active": processes.len()
        },
        "limits": {
            "maxSessions": state.config.max_sessions,
            "maxScanDepth": state.config.max_scan_depth,
            "maxFileReadBytes": state.config.max_file_read_bytes,
            "maxUploadFileBytes": MAX_UPLOAD_FILE_BYTES,
            "maxUploadFiles": MAX_UPLOAD_FILES
        }
    }))
}

async fn process_memory_metrics() -> Value {
    let Ok(status) = tokio::fs::read_to_string("/proc/self/status").await else {
        return serde_json::json!({
            "available": false
        });
    };
    let mut vm_rss_kb = None;
    let mut vm_size_kb = None;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            vm_rss_kb = parse_proc_status_kb(value);
        }
        if let Some(value) = line.strip_prefix("VmSize:") {
            vm_size_kb = parse_proc_status_kb(value);
        }
    }
    serde_json::json!({
        "available": true,
        "rssKb": vm_rss_kb,
        "virtualKb": vm_size_kb,
        "rssBytes": vm_rss_kb.map(|value| value * 1024),
        "virtualBytes": vm_size_kb.map(|value| value * 1024)
    })
}

fn parse_proc_status_kb(value: &str) -> Option<u64> {
    value
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
}

#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone)]
struct CpuSnapshot {
    aggregate: CpuTimes,
    cores: Vec<CpuTimes>,
}

#[derive(Debug, Clone)]
struct NetworkInterfaceSample {
    name: String,
    rx_bytes: u64,
    rx_packets: u64,
    rx_errors: u64,
    tx_bytes: u64,
    tx_packets: u64,
    tx_errors: u64,
}

async fn system_resource_metrics(workspace_root: &Path) -> Value {
    let previous_cpu = read_cpu_snapshot().await;
    let previous_network = read_network_snapshot().await;
    tokio::time::sleep(Duration::from_millis(RESOURCE_SAMPLE_INTERVAL_MS)).await;

    let current_cpu = read_cpu_snapshot().await;
    let current_network = read_network_snapshot().await;
    let memory = system_memory_metrics().await;
    let hardware = read_hardware_stats().await;
    let disk = disk_metrics(workspace_root).await;
    let load_average = read_load_average().await;
    let cpu_model = read_cpu_model().await;
    let system_uptime_seconds = read_system_uptime_seconds().await;
    let process_uptime_seconds = read_process_uptime_seconds(system_uptime_seconds).await;

    serde_json::json!({
        "cpu": cpu_metrics(previous_cpu, current_cpu, load_average, cpu_model),
        "memory": memory,
        "disk": disk,
        "network": network_metrics(previous_network, current_network),
        "hardware": hardware,
        "systemUptimeSeconds": system_uptime_seconds,
        "processUptimeSeconds": process_uptime_seconds,
    })
}

async fn read_text_path(path: impl AsRef<Path>) -> Option<String> {
    tokio::fs::read_to_string(path).await.ok()
}

async fn read_trimmed_path(path: impl AsRef<Path>) -> Option<String> {
    read_text_path(path)
        .await
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn read_sysfs_number(path: impl AsRef<Path>) -> Option<f64> {
    read_trimmed_path(path)
        .await
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

async fn read_directory_paths(directory: &str) -> Vec<PathBuf> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let is_directory_like = entry
            .file_type()
            .await
            .ok()
            .is_some_and(|file_type| file_type.is_dir() || file_type.is_symlink());
        if is_directory_like {
            paths.push(entry.path());
        }
    }
    paths
}

async fn read_directory_file_names(directory: &Path) -> Vec<String> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut names = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names
}

fn path_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

fn json_f64(value: Option<f64>) -> Value {
    value
        .filter(|value| value.is_finite())
        .map(Value::from)
        .unwrap_or(Value::Null)
}

fn json_u64(value: Option<u64>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}

fn parse_cpu_line(line: &str) -> Option<CpuTimes> {
    let mut parts = line.split_whitespace();
    let _name = parts.next()?;
    let values: Vec<u64> = parts
        .filter_map(|value| value.parse::<u64>().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().copied().sum();
    Some(CpuTimes { idle, total })
}

async fn read_cpu_snapshot() -> Option<CpuSnapshot> {
    let content = read_text_path("/proc/stat").await?;
    let aggregate = content
        .lines()
        .find(|line| line.starts_with("cpu "))
        .and_then(parse_cpu_line)?;
    let cores = content
        .lines()
        .filter(|line| {
            line.strip_prefix("cpu")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|ch| ch.is_ascii_digit())
        })
        .filter_map(parse_cpu_line)
        .collect();
    Some(CpuSnapshot { aggregate, cores })
}

fn calculate_cpu_percent(previous: CpuTimes, current: CpuTimes) -> Option<f64> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 {
        return None;
    }
    Some(((total_delta.saturating_sub(idle_delta)) as f64 / total_delta as f64) * 100.0)
}

fn cpu_metrics(
    previous: Option<CpuSnapshot>,
    current: Option<CpuSnapshot>,
    load_average: Vec<f64>,
    model: String,
) -> Value {
    let usage_percent = previous
        .as_ref()
        .zip(current.as_ref())
        .and_then(|(previous, current)| {
            calculate_cpu_percent(previous.aggregate, current.aggregate)
        });
    let per_core = current
        .as_ref()
        .map(|current| {
            current
                .cores
                .iter()
                .enumerate()
                .map(|(index, current_core)| {
                    let usage = previous
                        .as_ref()
                        .and_then(|previous| previous.cores.get(index).copied())
                        .and_then(|previous_core| {
                            calculate_cpu_percent(previous_core, *current_core)
                        });
                    serde_json::json!({
                        "index": index,
                        "usagePercent": json_f64(usage),
                        "temperatureCelsius": Value::Null,
                        "temperatureLabel": Value::Null,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cores = current
        .as_ref()
        .map(|snapshot| snapshot.cores.len())
        .unwrap_or(0);

    serde_json::json!({
        "usagePercent": json_f64(usage_percent),
        "processUsagePercent": Value::Null,
        "loadAverage": load_average,
        "cores": cores,
        "model": model,
        "perCore": per_core,
    })
}

async fn read_cpu_model() -> String {
    let Some(content) = read_text_path("/proc/cpuinfo").await else {
        return "Unknown CPU".to_string();
    };
    for key in ["model name", "Hardware", "Processor"] {
        if let Some(value) = content.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim() == key).then(|| value.trim().to_string())
        }) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    "Unknown CPU".to_string()
}

async fn read_load_average() -> Vec<f64> {
    read_trimmed_path("/proc/loadavg")
        .await
        .map(|content| {
            content
                .split_whitespace()
                .take(3)
                .filter_map(|value| value.parse::<f64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

async fn system_memory_metrics() -> Value {
    let Some(content) = read_text_path("/proc/meminfo").await else {
        return serde_json::json!({
            "total": 0,
            "used": 0,
            "free": 0,
            "available": 0,
            "usedPercent": Value::Null,
            "cached": 0,
            "buffers": 0,
            "swap": {
                "total": 0,
                "used": 0,
                "free": 0,
                "usedPercent": 0.0,
            }
        });
    };
    let mut values = HashMap::<String, u64>::new();
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if let Some(kb) = value
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
        {
            values.insert(key.to_string(), kb * 1024);
        }
    }

    let total = values.get("MemTotal").copied().unwrap_or(0);
    let available = values
        .get("MemAvailable")
        .copied()
        .unwrap_or_else(|| values.get("MemFree").copied().unwrap_or(0));
    let free = values.get("MemFree").copied().unwrap_or(available);
    let cached = values.get("Cached").copied().unwrap_or(0)
        + values.get("SReclaimable").copied().unwrap_or(0);
    let buffers = values.get("Buffers").copied().unwrap_or(0);
    let swap_total = values.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = values.get("SwapFree").copied().unwrap_or(0);
    let used = total.saturating_sub(available);
    let swap_used = swap_total.saturating_sub(swap_free);
    let used_percent = (total > 0).then(|| used as f64 / total as f64 * 100.0);
    let swap_percent = (swap_total > 0)
        .then(|| swap_used as f64 / swap_total as f64 * 100.0)
        .unwrap_or(0.0);

    serde_json::json!({
        "total": total,
        "used": used,
        "free": free,
        "available": available,
        "usedPercent": json_f64(used_percent),
        "cached": cached,
        "buffers": buffers,
        "swap": {
            "total": swap_total,
            "used": swap_used,
            "free": swap_free,
            "usedPercent": swap_percent,
        }
    })
}

async fn disk_metrics(path: &Path) -> Value {
    let output = timeout(
        Duration::from_secs(2),
        Command::new("df").arg("-PB1").arg(path).output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        return Value::Null;
    };
    if !output.status.success() {
        return Value::Null;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(line) = stdout.lines().nth(1) else {
        return Value::Null;
    };
    let columns: Vec<&str> = line.split_whitespace().collect();
    if columns.len() < 6 {
        return Value::Null;
    }
    let total = columns.get(1).and_then(|value| value.parse::<u64>().ok());
    let used = columns.get(2).and_then(|value| value.parse::<u64>().ok());
    let available = columns.get(3).and_then(|value| value.parse::<u64>().ok());
    let used_percent = total
        .zip(used)
        .and_then(|(total, used)| (total > 0).then(|| used as f64 / total as f64 * 100.0));

    serde_json::json!({
        "filesystem": columns[0],
        "mount": columns[5],
        "total": json_u64(total),
        "used": json_u64(used),
        "available": json_u64(available),
        "free": json_u64(available),
        "usedPercent": json_f64(used_percent),
    })
}

async fn read_network_snapshot() -> Option<Vec<NetworkInterfaceSample>> {
    let content = read_text_path("/proc/net/dev").await?;
    let mut interfaces = Vec::new();
    for line in content.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let numbers: Vec<u64> = values
            .split_whitespace()
            .filter_map(|value| value.parse::<u64>().ok())
            .collect();
        if numbers.len() < 16 {
            continue;
        }
        interfaces.push(NetworkInterfaceSample {
            name: name.to_string(),
            rx_bytes: numbers[0],
            rx_packets: numbers[1],
            rx_errors: numbers[2],
            tx_bytes: numbers[8],
            tx_packets: numbers[9],
            tx_errors: numbers[10],
        });
    }
    Some(interfaces)
}

fn network_metrics(
    previous: Option<Vec<NetworkInterfaceSample>>,
    current: Option<Vec<NetworkInterfaceSample>>,
) -> Value {
    let elapsed_seconds = RESOURCE_SAMPLE_INTERVAL_MS as f64 / 1000.0;
    let current = current.unwrap_or_default();
    let previous_by_name: HashMap<String, NetworkInterfaceSample> = previous
        .unwrap_or_default()
        .into_iter()
        .map(|sample| (sample.name.clone(), sample))
        .collect();
    let mut rx_bytes = 0_u64;
    let mut tx_bytes = 0_u64;
    let mut rx_rate = 0.0;
    let mut tx_rate = 0.0;
    let interfaces = current
        .into_iter()
        .map(|sample| {
            let previous = previous_by_name.get(&sample.name);
            let sample_rx_rate = previous
                .map(|previous| {
                    sample.rx_bytes.saturating_sub(previous.rx_bytes) as f64 / elapsed_seconds
                })
                .unwrap_or(0.0);
            let sample_tx_rate = previous
                .map(|previous| {
                    sample.tx_bytes.saturating_sub(previous.tx_bytes) as f64 / elapsed_seconds
                })
                .unwrap_or(0.0);
            rx_bytes = rx_bytes.saturating_add(sample.rx_bytes);
            tx_bytes = tx_bytes.saturating_add(sample.tx_bytes);
            rx_rate += sample_rx_rate;
            tx_rate += sample_tx_rate;
            serde_json::json!({
                "name": sample.name,
                "rxBytes": sample.rx_bytes,
                "txBytes": sample.tx_bytes,
                "rxRateBytesPerSecond": sample_rx_rate,
                "txRateBytesPerSecond": sample_tx_rate,
                "rxPackets": sample.rx_packets,
                "txPackets": sample.tx_packets,
                "rxErrors": sample.rx_errors,
                "txErrors": sample.tx_errors,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "rxBytes": rx_bytes,
        "txBytes": tx_bytes,
        "rxRateBytesPerSecond": rx_rate,
        "txRateBytesPerSecond": tx_rate,
        "interfaces": interfaces,
    })
}

async fn read_system_uptime_seconds() -> Option<u64> {
    read_trimmed_path("/proc/uptime")
        .await
        .and_then(|content| content.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.floor() as u64)
}

async fn read_process_uptime_seconds(system_uptime_seconds: Option<u64>) -> Option<u64> {
    let system_uptime_seconds = system_uptime_seconds?;
    let stat = read_trimmed_path("/proc/self/stat").await?;
    let after_command = stat.rsplit_once(')')?.1.trim();
    let fields: Vec<&str> = after_command.split_whitespace().collect();
    let start_ticks = fields.get(19)?.parse::<u64>().ok()?;
    let start_seconds = start_ticks / clock_ticks_per_second().max(1);
    Some(system_uptime_seconds.saturating_sub(start_seconds))
}

fn clock_ticks_per_second() -> u64 {
    env::var("IO_WORKBENCH_CLK_TCK")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100)
}

fn normalize_temperature_celsius(raw_value: Option<f64>) -> Option<f64> {
    let raw_value = raw_value?;
    let celsius = if raw_value.abs() > 1000.0 {
        raw_value / 1000.0
    } else {
        raw_value
    };
    (-100.0..=250.0).contains(&celsius).then_some(celsius)
}

async fn read_thermal_zone_temperatures() -> Vec<Value> {
    let mut sensors = Vec::new();
    for zone_path in read_directory_paths(SYS_THERMAL_PATH).await {
        let name = path_file_name(&zone_path);
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let raw_temp = read_sysfs_number(zone_path.join("temp")).await;
        let Some(celsius) = normalize_temperature_celsius(raw_temp) else {
            continue;
        };
        let label = read_trimmed_path(zone_path.join("type"))
            .await
            .unwrap_or_else(|| name.clone());
        sensors.push(serde_json::json!({
            "id": name,
            "label": label,
            "celsius": celsius,
            "source": "thermal",
            "path": zone_path.display().to_string(),
        }));
    }
    sensors
}

async fn read_hwmon_stats() -> (Vec<Value>, Vec<Value>) {
    let mut temperature_sensors = Vec::new();
    let mut fans = Vec::new();
    for hwmon_path in read_directory_paths(SYS_HWMON_PATH).await {
        let name = path_file_name(&hwmon_path);
        if !name.starts_with("hwmon") {
            continue;
        }
        let hwmon_name = read_trimmed_path(hwmon_path.join("name")).await;
        let files = read_directory_file_names(&hwmon_path).await;
        for file_name in files {
            if let Some(index) = file_name
                .strip_prefix("temp")
                .and_then(|value| value.strip_suffix("_input"))
            {
                let raw_temp = read_sysfs_number(hwmon_path.join(&file_name)).await;
                let Some(celsius) = normalize_temperature_celsius(raw_temp) else {
                    continue;
                };
                let label = read_trimmed_path(hwmon_path.join(format!("temp{index}_label")))
                    .await
                    .or_else(|| hwmon_name.clone())
                    .unwrap_or_else(|| format!("Temperature {index}"));
                temperature_sensors.push(serde_json::json!({
                    "id": format!("{name}:temp{index}"),
                    "label": label,
                    "celsius": celsius,
                    "source": "hwmon",
                    "path": hwmon_path.display().to_string(),
                }));
                continue;
            }

            if let Some(index) = file_name
                .strip_prefix("fan")
                .and_then(|value| value.strip_suffix("_input"))
            {
                let Some(rpm) = read_sysfs_number(hwmon_path.join(&file_name)).await else {
                    continue;
                };
                let label = read_trimmed_path(hwmon_path.join(format!("fan{index}_label")))
                    .await
                    .or_else(|| hwmon_name.clone())
                    .unwrap_or_else(|| format!("Fan {index}"));
                let fault = read_sysfs_number(hwmon_path.join(format!("fan{index}_fault")))
                    .await
                    .unwrap_or(0.0);
                let alarm = read_sysfs_number(hwmon_path.join(format!("fan{index}_alarm")))
                    .await
                    .unwrap_or(0.0);
                let status = if fault > 0.0 || alarm > 0.0 {
                    "fault"
                } else if rpm > 0.0 {
                    "ok"
                } else {
                    "stopped"
                };
                fans.push(serde_json::json!({
                    "id": format!("{name}:fan{index}"),
                    "label": label,
                    "rpm": rpm.max(0.0),
                    "status": status,
                    "source": "hwmon",
                    "path": hwmon_path.display().to_string(),
                }));
            }
        }
    }
    (temperature_sensors, fans)
}

fn temperature_value(sensor: &Value) -> f64 {
    sensor.get("celsius").and_then(Value::as_f64).unwrap_or(0.0)
}

fn temperature_sensor_score(sensor: &Value) -> i32 {
    let label = format!(
        "{} {}",
        sensor
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        sensor.get("id").and_then(Value::as_str).unwrap_or_default()
    )
    .to_ascii_lowercase();
    let mut score = 0;
    for token in ["cpu", "processor", "coretemp", "k10temp", "zenpower"] {
        if label.contains(token) {
            score += 5;
        }
    }
    for token in ["package", "x86_pkg", "tctl", "tdie"] {
        if label.contains(token) {
            score += 4;
        }
    }
    if label.contains("core") {
        score += 2;
    }
    for token in ["nvme", "gpu", "wifi", "pch"] {
        if label.contains(token) {
            score -= 4;
        }
    }
    score
}

fn select_processor_temperature(sensors: &[Value]) -> Value {
    sensors
        .iter()
        .filter_map(|sensor| {
            let score = temperature_sensor_score(sensor);
            (score > 0).then_some((score, sensor))
        })
        .max_by(|(left_score, left), (right_score, right)| {
            left_score.cmp(right_score).then_with(|| {
                temperature_value(left)
                    .partial_cmp(&temperature_value(right))
                    .unwrap_or(Ordering::Equal)
            })
        })
        .map(|(_, sensor)| sensor.clone())
        .unwrap_or(Value::Null)
}

async fn read_battery_stats() -> Vec<Value> {
    let mut batteries = Vec::new();
    for battery_path in read_directory_paths(SYS_POWER_SUPPLY_PATH).await {
        let type_value = read_trimmed_path(battery_path.join("type")).await;
        if !type_value
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("battery"))
        {
            continue;
        }
        let raw_capacity = read_sysfs_number(battery_path.join("capacity")).await;
        let energy_now = read_sysfs_number(battery_path.join("energy_now")).await;
        let energy_full = read_sysfs_number(battery_path.join("energy_full")).await;
        let charge_now = read_sysfs_number(battery_path.join("charge_now")).await;
        let charge_full = read_sysfs_number(battery_path.join("charge_full")).await;
        let energy_percent = energy_now
            .zip(energy_full)
            .and_then(|(now, full)| (full > 0.0).then(|| now / full * 100.0));
        let charge_percent = charge_now
            .zip(charge_full)
            .and_then(|(now, full)| (full > 0.0).then(|| now / full * 100.0));
        let level_percent = raw_capacity
            .or(energy_percent)
            .or(charge_percent)
            .map(|value| value.clamp(0.0, 100.0));
        batteries.push(serde_json::json!({
            "name": path_file_name(&battery_path),
            "levelPercent": json_f64(level_percent),
            "status": read_trimmed_path(battery_path.join("status")).await,
            "manufacturer": read_trimmed_path(battery_path.join("manufacturer")).await,
            "model": read_trimmed_path(battery_path.join("model_name")).await,
            "technology": read_trimmed_path(battery_path.join("technology")).await,
            "path": battery_path.display().to_string(),
        }));
    }
    batteries
}

async fn read_hardware_stats() -> Value {
    let (mut hwmon_temperatures, fans) = read_hwmon_stats().await;
    hwmon_temperatures.extend(read_thermal_zone_temperatures().await);
    hwmon_temperatures.sort_by(|left, right| {
        temperature_value(right)
            .partial_cmp(&temperature_value(left))
            .unwrap_or(Ordering::Equal)
    });
    let processor_temperature = select_processor_temperature(&hwmon_temperatures);
    let temperature_sensors = hwmon_temperatures.into_iter().take(16).collect::<Vec<_>>();
    serde_json::json!({
        "processorTemperature": processor_temperature,
        "temperatureSensors": temperature_sensors,
        "fans": fans,
        "batteries": read_battery_stats().await,
    })
}

#[derive(Clone)]
pub(crate) struct AuthenticatedUser(pub iowb_protocol::UserProfile);
