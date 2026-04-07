use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
struct ListeningEntry {
    port: u16,
    pid: u32,
    process_name: String,
}

#[derive(Clone, Debug, Default)]
struct ProcessSnapshot {
    ppid: u32,
    stat: String,
    rss_kb: u64,
    elapsed_secs: Option<u64>,
    command: String,
}

#[derive(Clone, Debug)]
struct PortInfo {
    port: u16,
    pid: u32,
    process_name: String,
    command: String,
    cwd: Option<PathBuf>,
    project_name: Option<String>,
    framework: Option<String>,
    uptime: Option<String>,
    status: String,
    memory: Option<String>,
    process_tree: Vec<ProcessTreeNode>,
}

#[derive(Clone, Debug)]
struct ProcessInfo {
    pid: u32,
    process_name: String,
    command: String,
    description: String,
    cpu: f64,
    memory: Option<String>,
    cwd: Option<PathBuf>,
    project_name: Option<String>,
    framework: Option<String>,
    uptime: Option<String>,
}

#[derive(Clone, Debug)]
struct ProcessListEntry {
    pid: u32,
    process_name: String,
    cpu: f64,
    rss_kb: u64,
    elapsed_secs: Option<u64>,
    command: String,
}

#[derive(Clone, Debug)]
struct ProcessTreeNode {
    pid: u32,
    ppid: u32,
    name: String,
}

#[derive(Clone, Debug)]
struct DockerInfo {
    name: String,
    image: String,
}

#[derive(Clone, Debug)]
enum KillTarget {
    Port { port: u16, info: PortInfo },
    Pid(u32),
}

fn main() {
    if let Err(err) = run() {
        eprintln!("\nError: {err}\n");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let show_all = raw_args.iter().any(|arg| arg == "--all" || arg == "-a");
    let filtered_args: Vec<String> = raw_args
        .iter()
        .filter(|arg| arg.as_str() != "--all" && arg.as_str() != "-a")
        .cloned()
        .collect();
    let command = filtered_args.first().cloned();

    match command.as_deref() {
        None => {
            let mut ports = get_listening_ports(false)?;
            if !show_all {
                ports.retain(|p| is_dev_process(&p.process_name, &p.command));
            }
            display_port_table(&ports, !show_all);
        }
        Some("ps") => {
            let mut processes = get_all_processes()?;
            if !show_all {
                processes.retain(|p| is_dev_process(&p.process_name, &p.command));
                processes = collapse_docker_processes(processes);
            }
            processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            display_process_table(&processes, !show_all);
        }
        Some("clean") => {
            let orphaned = find_orphaned_processes()?;
            if orphaned.is_empty() {
                display_clean_results(&orphaned, &[], &[]);
                return Ok(());
            }

            println!();
            println!(
                "Found {} orphaned/zombie process{}:",
                orphaned.len(),
                if orphaned.len() == 1 { "" } else { "es" }
            );
            for p in &orphaned {
                println!("  * :{} - {} (PID {})", p.port, p.process_name, p.pid);
            }
            println!();

            if confirm("Kill all? [y/N] ")? {
                let mut killed = Vec::new();
                let mut failed = Vec::new();
                for p in &orphaned {
                    if kill_process(p.pid, false) {
                        killed.push(p.pid);
                    } else {
                        failed.push(p.pid);
                    }
                }
                display_clean_results(&orphaned, &killed, &failed);
            } else {
                println!("Aborted.\n");
            }
        }
        Some("kill") => {
            let force = filtered_args.iter().any(|arg| arg == "--force" || arg == "-f");
            let kill_args: Vec<&String> = filtered_args
                .iter()
                .skip(1)
                .filter(|arg| arg.as_str() != "--force" && arg.as_str() != "-f")
                .collect();

            if kill_args.is_empty() {
                return Err("Usage: ports-rs kill [-f|--force] <port|pid> [port|pid...]".into());
            }

            let mut any_failed = false;
            println!();

            for arg in kill_args {
                let Ok(value) = arg.parse::<u32>() else {
                    println!("  x \"{arg}\" is not a valid port/PID");
                    any_failed = true;
                    continue;
                };

                match resolve_kill_target(value)? {
                    Some(KillTarget::Port { port, info }) => {
                        let label = format!(":{} - {} (PID {})", port, info.process_name, info.pid);
                        println!("  Killing {label}");
                        if kill_process(info.pid, force) {
                            println!("  ok sent {} to {label}", if force { "SIGKILL" } else { "SIGTERM" });
                        } else {
                            println!("  x failed to kill {label}");
                            any_failed = true;
                        }
                    }
                    Some(KillTarget::Pid(pid)) => {
                        let label = format!("PID {pid}");
                        println!("  Killing {label}");
                        if kill_process(pid, force) {
                            println!("  ok sent {} to {label}", if force { "SIGKILL" } else { "SIGTERM" });
                        } else {
                            println!("  x failed to kill {label}");
                            any_failed = true;
                        }
                    }
                    None => {
                        println!("  x no listener or process found for {value}");
                        any_failed = true;
                    }
                }
            }

            println!();
            if any_failed {
                process::exit(1);
            }
        }
        Some("watch") => watch_ports()?,
        Some("help") | Some("--help") | Some("-h") => display_help(),
        Some(other) => {
            if let Ok(port) = other.parse::<u16>() {
                let info = get_port_details(port)?;
                display_port_detail(info.as_ref());
                if let Some(info) = info {
                    if confirm(&format!("Kill process on :{}? [y/N] ", port))? {
                        if kill_process(info.pid, false) {
                            println!("\nKilled PID {}\n", info.pid);
                        } else {
                            println!("\nFailed to kill PID {}\n", info.pid);
                        }
                    }
                }
            } else {
                return Err(format!("Unknown command: {other}. Run ports-rs --help."));
            }
        }
    }

    Ok(())
}

fn display_help() {
    println!();
    println!("Port Whisperer (Rust)");
    println!();
    println!("Usage:");
    println!("  ports-rs              Show dev server ports");
    println!("  ports-rs --all        Show all listening ports");
    println!("  ports-rs ps           Show all running dev processes");
    println!("  ports-rs <number>     Detailed info about a specific port");
    println!("  ports-rs kill <n>     Kill by port or PID (-f for force)");
    println!("  ports-rs clean        Kill orphaned/zombie dev servers");
    println!("  ports-rs watch        Monitor port changes in real-time");
    println!();
}

fn confirm(prompt: &str) -> Result<bool, String> {
    print!("{prompt}");
    io::stdout().flush().map_err(|err| err.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|err| err.to_string())?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

fn get_listening_ports(detailed: bool) -> Result<Vec<PortInfo>, String> {
    let entries = get_listening_ports_raw()?;
    let unique_pids: Vec<u32> = entries
        .iter()
        .map(|entry| entry.pid)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let ps_map = batch_process_info(&unique_pids)?;
    let cwd_map = batch_cwd(&unique_pids)?;
    let has_docker = entries.iter().any(|entry| is_docker_process_name(&entry.process_name));
    let docker_map = if has_docker {
        batch_docker_info()
    } else {
        HashMap::new()
    };

    let mut results = Vec::new();

    for entry in entries {
        let ps = ps_map.get(&entry.pid).cloned().unwrap_or_default();
        let cwd = cwd_map.get(&entry.pid).cloned();

        let mut info = PortInfo {
            port: entry.port,
            pid: entry.pid,
            process_name: entry.process_name.clone(),
            command: ps.command.clone(),
            cwd: None,
            project_name: None,
            framework: None,
            uptime: ps.elapsed_secs.map(format_uptime),
            status: "healthy".into(),
            memory: if ps.rss_kb > 0 {
                Some(format_memory(ps.rss_kb))
            } else {
                None
            },
            process_tree: Vec::new(),
        };

        if ps.stat.contains('Z') {
            info.status = "zombie".into();
        } else if ps.ppid == 1 && is_dev_process(&entry.process_name, &ps.command) {
            info.status = "orphaned".into();
        }

        info.framework = detect_framework_from_command(&ps.command, &entry.process_name);

        if let Some(docker) = docker_map.get(&entry.port) {
            info.project_name = Some(docker.name.clone());
            info.framework = Some(detect_framework_from_image(&docker.image));
            info.process_name = "docker".into();
        } else if let Some(cwd) = cwd {
            let project_root = find_project_root(&cwd);
            info.cwd = Some(project_root.clone());
            info.project_name = project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string());
            if info.framework.is_none() {
                info.framework = detect_framework(&project_root);
            }
        }

        if detailed {
            info.process_tree = get_process_tree(info.pid)?;
        }

        results.push(info);
    }

    results.sort_by_key(|info| info.port);
    Ok(results)
}

fn get_port_details(target_port: u16) -> Result<Option<PortInfo>, String> {
    let ports = get_listening_ports(true)?;
    Ok(ports.into_iter().find(|port| port.port == target_port))
}

fn get_all_processes() -> Result<Vec<ProcessInfo>, String> {
    let entries = get_all_processes_raw()?;
    let non_docker_pids: Vec<u32> = entries
        .iter()
        .filter(|entry| !is_docker_process_name(&entry.process_name))
        .map(|entry| entry.pid)
        .collect();
    let cwd_map = batch_cwd(&non_docker_pids)?;

    Ok(entries
        .into_iter()
        .map(|entry| {
            let cwd = cwd_map.get(&entry.pid).cloned();
            let mut info = ProcessInfo {
                pid: entry.pid,
                process_name: entry.process_name.clone(),
                command: entry.command.clone(),
                description: summarize_command(&entry.command, &entry.process_name),
                cpu: entry.cpu,
                memory: if entry.rss_kb > 0 {
                    Some(format_memory(entry.rss_kb))
                } else {
                    None
                },
                cwd: None,
                project_name: None,
                framework: detect_framework_from_command(&entry.command, &entry.process_name),
                uptime: entry.elapsed_secs.map(format_uptime),
            };

            if let Some(cwd) = cwd {
                let project_root = find_project_root(&cwd);
                info.cwd = Some(project_root.clone());
                info.project_name = project_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_string());
                if info.framework.is_none() {
                    info.framework = detect_framework(&project_root);
                }
            }

            info
        })
        .collect())
}

fn find_orphaned_processes() -> Result<Vec<PortInfo>, String> {
    Ok(get_listening_ports(false)?
        .into_iter()
        .filter(|port| port.status == "orphaned" || port.status == "zombie")
        .collect())
}

fn resolve_kill_target(value: u32) -> Result<Option<KillTarget>, String> {
    if value == 0 {
        return Ok(None);
    }

    if value <= u16::MAX as u32 {
        let port = value as u16;
        if let Some(info) = get_port_details(port)? {
            return Ok(Some(KillTarget::Port { port, info }));
        }
    }

    if pid_exists(value) {
        return Ok(Some(KillTarget::Pid(value)));
    }

    Ok(None)
}

fn pid_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"
                ),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn kill_process(pid: u32, force: bool) -> bool {
    #[cfg(unix)]
    {
        let mut cmd = Command::new("kill");
        if force {
            cmd.arg("-9");
        }
        cmd.arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        if force {
            cmd.arg("/F");
        }
        cmd.args(["/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn watch_ports() -> Result<(), String> {
    println!();
    println!("Watching for port changes...");
    println!("Press Ctrl+C to stop.");
    println!();

    let mut previous_ports = HashSet::new();

    loop {
        let current = get_listening_ports(false)?;
        let current_ports: HashSet<u16> = current.iter().map(|port| port.port).collect();

        for port in &current {
            if !previous_ports.contains(&port.port) {
                println!(
                    "[new] :{} <- {}{}{}",
                    port.port,
                    port.process_name,
                    port.project_name
                        .as_ref()
                        .map(|name| format!(" [{name}]"))
                        .unwrap_or_default(),
                    port.framework
                        .as_ref()
                        .map(|framework| format!(" {framework}"))
                        .unwrap_or_default()
                );
            }
        }

        for port in previous_ports.difference(&current_ports) {
            println!("[closed] :{port}");
        }

        previous_ports = current_ports;
        thread::sleep(Duration::from_secs(2));
    }
}

fn display_port_table(ports: &[PortInfo], filtered: bool) {
    print_header();
    if ports.is_empty() {
        println!("No active listening ports found.\n");
        return;
    }

    let rows: Vec<Vec<String>> = ports
        .iter()
        .map(|port| {
            vec![
                format!(":{}", port.port),
                port.process_name.clone(),
                port.pid.to_string(),
                port.project_name.clone().unwrap_or_else(|| "-".into()),
                port.framework.clone().unwrap_or_else(|| "-".into()),
                port.uptime.clone().unwrap_or_else(|| "-".into()),
                port.status.clone(),
            ]
        })
        .collect();

    print_table(
        &["PORT", "PROCESS", "PID", "PROJECT", "FRAMEWORK", "UPTIME", "STATUS"],
        &rows,
    );

    println!();
    print!(
        "{} port{} active  |  Run ports-rs <number> for details",
        ports.len(),
        if ports.len() == 1 { "" } else { "s" }
    );
    if filtered {
        print!("  |  --all to show everything");
    }
    println!("\n");
}

fn display_process_table(processes: &[ProcessInfo], filtered: bool) {
    print_header();
    if processes.is_empty() {
        println!("No dev processes found.\n");
        return;
    }

    let rows: Vec<Vec<String>> = processes
        .iter()
        .map(|proc| {
            vec![
                proc.pid.to_string(),
                proc.process_name.clone(),
                format!("{:.1}", proc.cpu),
                proc.memory.clone().unwrap_or_else(|| "-".into()),
                proc.project_name.clone().unwrap_or_else(|| "-".into()),
                proc.framework.clone().unwrap_or_else(|| "-".into()),
                proc.uptime.clone().unwrap_or_else(|| "-".into()),
                proc.description.clone(),
            ]
        })
        .collect();

    print_table(
        &["PID", "PROCESS", "CPU%", "MEM", "PROJECT", "FRAMEWORK", "UPTIME", "WHAT"],
        &rows,
    );

    println!();
    print!(
        "{} process{}",
        processes.len(),
        if processes.len() == 1 { "" } else { "es" }
    );
    if filtered {
        print!("  |  --all to show everything");
    }
    println!("\n");
}

fn display_port_detail(info: Option<&PortInfo>) {
    print_header();
    let Some(info) = info else {
        println!("No process found on that port.\n");
        return;
    };

    println!("Port :{}", info.port);
    println!("----------------------");
    println!("Process    {}", info.process_name);
    println!("PID        {}", info.pid);
    println!("Status     {}", info.status);
    println!(
        "Framework  {}",
        info.framework.clone().unwrap_or_else(|| "-".into())
    );
    println!("Memory     {}", info.memory.clone().unwrap_or_else(|| "-".into()));
    println!("Uptime     {}", info.uptime.clone().unwrap_or_else(|| "-".into()));
    println!();
    println!("Location");
    println!("----------------------");
    println!(
        "Directory  {}",
        info.cwd
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "Project    {}",
        info.project_name.clone().unwrap_or_else(|| "-".into())
    );

    if !info.process_tree.is_empty() {
        println!();
        println!("Process Tree");
        println!("----------------------");
        for (idx, node) in info.process_tree.iter().enumerate() {
            let prefix = if idx == 0 { "->" } else { "\\-" };
            let indent = "  ".repeat(idx);
            println!("{indent}{prefix} {} ({})", node.name, node.pid);
        }
    }

    println!();
    println!("Kill: ports-rs kill {} or ports-rs kill -f {}", info.port, info.port);
    println!();
}

fn display_clean_results(orphaned: &[PortInfo], killed: &[u32], failed: &[u32]) {
    print_header();
    if orphaned.is_empty() {
        println!("No orphaned or zombie processes found. All clean.\n");
        return;
    }

    println!(
        "Found {} orphaned/zombie process{}:\n",
        orphaned.len(),
        if orphaned.len() == 1 { "" } else { "es" }
    );

    for port in orphaned {
        let icon = if killed.contains(&port.pid) {
            "ok"
        } else if failed.contains(&port.pid) {
            "x"
        } else {
            "?"
        };
        println!("  {icon} :{} - {} (PID {})", port.port, port.process_name, port.pid);
    }

    if !killed.is_empty() {
        println!(
            "\nCleaned {} process{}.",
            killed.len(),
            if killed.len() == 1 { "" } else { "es" }
        );
    }
    if !failed.is_empty() {
        println!(
            "Failed to clean {} process{}.",
            failed.len(),
            if failed.len() == 1 { "" } else { "es" }
        );
    }
    println!();
}

fn print_header() {
    println!();
    println!("+-------------------------------------+");
    println!("| Port Whisperer (Rust)               |");
    println!("| listening to your ports...          |");
    println!("+-------------------------------------+");
    println!();
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.len());
        }
    }

    print_separator(&widths);
    print_row(headers, &widths);
    print_separator(&widths);
    for row in rows {
        let cells: Vec<&str> = row.iter().map(|cell| cell.as_str()).collect();
        print_row(&cells, &widths);
    }
    print_separator(&widths);
}

fn print_separator(widths: &[usize]) {
    print!("+");
    for width in widths {
        print!("{}+", "-".repeat(*width + 2));
    }
    println!();
}

fn print_row(cells: &[&str], widths: &[usize]) {
    print!("|");
    for (cell, width) in cells.iter().zip(widths.iter()) {
        print!(" {:width$} |", cell, width = *width);
    }
    println!();
}

fn collapse_docker_processes(processes: Vec<ProcessInfo>) -> Vec<ProcessInfo> {
    let mut docker = Vec::new();
    let mut other = Vec::new();

    for process in processes {
        if is_docker_process_name(&process.process_name) {
            docker.push(process);
        } else {
            other.push(process);
        }
    }

    if docker.is_empty() {
        return other;
    }

    let total_cpu = docker.iter().map(|proc| proc.cpu).sum::<f64>();
    let total_kb = docker
        .iter()
        .filter_map(|proc| proc.memory.as_deref())
        .map(parse_memory_kb)
        .sum::<u64>();

    other.push(ProcessInfo {
        pid: docker[0].pid,
        process_name: "Docker".into(),
        command: String::new(),
        description: format!("{} processes", docker.len()),
        cpu: total_cpu,
        memory: if total_kb == 0 {
            None
        } else {
            Some(format_memory(total_kb))
        },
        cwd: None,
        project_name: None,
        framework: Some("Docker".into()),
        uptime: docker[0].uptime.clone(),
    });

    other
}

fn parse_memory_kb(value: &str) -> u64 {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 2 {
        return 0;
    }
    let amount = parts[0].parse::<f64>().unwrap_or(0.0);
    match parts[1] {
        "GB" => (amount * 1_048_576.0) as u64,
        "MB" => (amount * 1_024.0) as u64,
        "KB" => amount as u64,
        _ => 0,
    }
}

fn is_dev_process(process_name: &str, command: &str) -> bool {
    let name = process_name.to_ascii_lowercase();
    let cmd = command.to_ascii_lowercase();

    let system_apps = [
        "spotify",
        "raycast",
        "tableplus",
        "postman",
        "linear",
        "cursor",
        "slack",
        "discord",
        "firefox",
        "chrome",
        "google",
        "safari",
        "figma",
        "notion",
        "zoom",
        "teams",
        "code",
        "iterm2",
        "warp",
        "arc",
        "loginwindow",
        "windowserver",
        "systemuiserver",
        "kernel_task",
        "launchd",
        "systemd",
        "snapd",
        "networkmanager",
        "gdm",
        "sshd",
        "cron",
        "svchost",
        "csrss",
        "lsass",
        "services",
        "explorer",
        "dwm",
    ];
    if system_apps.iter().any(|app| name.starts_with(app)) {
        return false;
    }

    let dev_names = [
        "node",
        "python",
        "python3",
        "ruby",
        "java",
        "go",
        "cargo",
        "deno",
        "bun",
        "php",
        "uvicorn",
        "gunicorn",
        "flask",
        "rails",
        "npm",
        "npx",
        "yarn",
        "pnpm",
        "tsc",
        "tsx",
        "esbuild",
        "rollup",
        "turbo",
        "nx",
        "jest",
        "vitest",
        "mocha",
        "pytest",
        "cypress",
        "playwright",
        "rustc",
        "dotnet",
        "gradle",
        "mvn",
        "mix",
        "elixir",
    ];
    if dev_names.contains(&name.as_str()) {
        return true;
    }

    if is_docker_process_name(&name) {
        return true;
    }

    let indicators = [
        " node ",
        "next ",
        "vite",
        "nuxt",
        "webpack",
        "remix",
        "astro",
        "django",
        "manage.py",
        "uvicorn",
        "rails",
        "cargo",
    ];
    indicators.iter().any(|indicator| cmd.contains(indicator))
}

fn is_docker_process_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("com.docke") || lower == "docker" || lower == "docker-sandbox"
}

fn summarize_command(command: &str, process_name: &str) -> String {
    let mut meaningful = Vec::new();
    for (idx, part) in command.split_whitespace().enumerate() {
        if idx == 0 || part.starts_with('-') {
            continue;
        }
        if let Some(name) = Path::new(part).file_name().and_then(|name| name.to_str()) {
            meaningful.push(name.to_string());
        } else {
            meaningful.push(part.to_string());
        }
        if meaningful.len() == 3 {
            break;
        }
    }

    if meaningful.is_empty() {
        process_name.to_string()
    } else {
        meaningful.join(" ")
    }
}

fn detect_framework_from_image(image: &str) -> String {
    let image = image.to_ascii_lowercase();
    if image.contains("postgres") {
        "PostgreSQL".into()
    } else if image.contains("redis") {
        "Redis".into()
    } else if image.contains("mysql") || image.contains("mariadb") {
        "MySQL".into()
    } else if image.contains("mongo") {
        "MongoDB".into()
    } else if image.contains("nginx") {
        "nginx".into()
    } else if image.contains("localstack") {
        "LocalStack".into()
    } else if image.contains("rabbitmq") {
        "RabbitMQ".into()
    } else if image.contains("kafka") {
        "Kafka".into()
    } else if image.contains("elasticsearch") || image.contains("opensearch") {
        "Elasticsearch".into()
    } else if image.contains("minio") {
        "MinIO".into()
    } else {
        "Docker".into()
    }
}

fn detect_framework(project_root: &Path) -> Option<String> {
    let package_json = project_root.join("package.json");
    if let Ok(contents) = fs::read_to_string(&package_json) {
        let checks = [
            ("\"next\"", "Next.js"),
            ("\"nuxt\"", "Nuxt"),
            ("\"nuxt3\"", "Nuxt"),
            ("\"@sveltejs/kit\"", "SvelteKit"),
            ("\"svelte\"", "Svelte"),
            ("\"@remix-run/react\"", "Remix"),
            ("\"remix\"", "Remix"),
            ("\"astro\"", "Astro"),
            ("\"vite\"", "Vite"),
            ("\"@angular/core\"", "Angular"),
            ("\"vue\"", "Vue"),
            ("\"react\"", "React"),
            ("\"express\"", "Express"),
            ("\"fastify\"", "Fastify"),
            ("\"hono\"", "Hono"),
            ("\"koa\"", "Koa"),
            ("\"nestjs\"", "NestJS"),
            ("\"@nestjs/core\"", "NestJS"),
            ("\"gatsby\"", "Gatsby"),
            ("\"webpack-dev-server\"", "Webpack"),
            ("\"esbuild\"", "esbuild"),
            ("\"parcel\"", "Parcel"),
        ];
        for (needle, framework) in checks {
            if contents.contains(needle) {
                return Some(framework.into());
            }
        }
    }

    let file_markers = [
        ("vite.config.ts", "Vite"),
        ("vite.config.js", "Vite"),
        ("next.config.js", "Next.js"),
        ("next.config.mjs", "Next.js"),
        ("angular.json", "Angular"),
        ("Cargo.toml", "Rust"),
        ("go.mod", "Go"),
        ("manage.py", "Django"),
        ("Gemfile", "Ruby"),
    ];
    for (file, framework) in file_markers {
        if project_root.join(file).exists() {
            return Some(framework.into());
        }
    }

    None
}

fn detect_framework_from_command(command: &str, process_name: &str) -> Option<String> {
    let command = command.to_ascii_lowercase();
    if command.contains("next") {
        Some("Next.js".into())
    } else if command.contains("vite") {
        Some("Vite".into())
    } else if command.contains("nuxt") {
        Some("Nuxt".into())
    } else if command.contains("angular") || command.contains("ng serve") {
        Some("Angular".into())
    } else if command.contains("webpack") {
        Some("Webpack".into())
    } else if command.contains("remix") {
        Some("Remix".into())
    } else if command.contains("astro") {
        Some("Astro".into())
    } else if command.contains("gatsby") {
        Some("Gatsby".into())
    } else if command.contains("flask") {
        Some("Flask".into())
    } else if command.contains("django") || command.contains("manage.py") {
        Some("Django".into())
    } else if command.contains("uvicorn") {
        Some("FastAPI".into())
    } else if command.contains("rails") {
        Some("Rails".into())
    } else if command.contains("cargo") || command.contains("rustc") {
        Some("Rust".into())
    } else {
        detect_framework_from_name(process_name)
    }
}

fn detect_framework_from_name(process_name: &str) -> Option<String> {
    match process_name.to_ascii_lowercase().as_str() {
        "node" => Some("Node.js".into()),
        "python" | "python3" => Some("Python".into()),
        "ruby" => Some("Ruby".into()),
        "java" => Some("Java".into()),
        "go" => Some("Go".into()),
        _ => None,
    }
}

fn find_project_root(dir: &Path) -> PathBuf {
    let markers = [
        "package.json",
        "Cargo.toml",
        "go.mod",
        "pyproject.toml",
        "Gemfile",
        "pom.xml",
        "build.gradle",
    ];

    let mut current = dir.to_path_buf();
    for _ in 0..15 {
        if markers.iter().any(|marker| current.join(marker).exists()) {
            return current;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    dir.to_path_buf()
}

fn batch_docker_info() -> HashMap<u16, DockerInfo> {
    let mut map = HashMap::new();
    let Ok(raw) = run_command("docker", &["ps", "--format", "{{.Ports}}\t{{.Names}}\t{{.Image}}"]) else {
        return map;
    };

    for line in raw.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let ports_str = parts[0];
        let name = parts[1].to_string();
        let image = parts[2].to_string();

        for segment in ports_str.split(',') {
            if let Some(port) = extract_host_port(segment) {
                map.entry(port).or_insert_with(|| DockerInfo {
                    name: name.clone(),
                    image: image.clone(),
                });
            }
        }
    }

    map
}

fn extract_host_port(segment: &str) -> Option<u16> {
    let before_arrow = segment.split("->").next()?;
    let candidate = before_arrow
        .rsplit(':')
        .next()?
        .trim()
        .trim_matches(|ch| ch == '[' || ch == ']');
    candidate.parse::<u16>().ok()
}

fn get_listening_ports_raw() -> Result<Vec<ListeningEntry>, String> {
    #[cfg(target_os = "macos")]
    {
        get_listening_ports_raw_macos()
    }
    #[cfg(target_os = "linux")]
    {
        get_listening_ports_raw_linux()
    }
    #[cfg(target_os = "windows")]
    {
        get_listening_ports_raw_windows()
    }
}

#[cfg(target_os = "macos")]
fn get_listening_ports_raw_macos() -> Result<Vec<ListeningEntry>, String> {
    let raw = run_command_allow_nonzero("lsof", &["-iTCP", "-sTCP:LISTEN", "-P", "-n"])?;
    let mut entries = Vec::new();
    let mut seen_ports = HashSet::new();

    for line in raw.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let process_name = parts[0].to_string();
        let Ok(pid) = parts[1].parse::<u32>() else {
            continue;
        };
        let Some(port) = parse_port(parts[8]) else {
            continue;
        };
        if seen_ports.insert(port) {
            entries.push(ListeningEntry {
                port,
                pid,
                process_name,
            });
        }
    }

    Ok(entries)
}

#[cfg(target_os = "linux")]
fn get_listening_ports_raw_linux() -> Result<Vec<ListeningEntry>, String> {
    let mut entries = Vec::new();
    let mut seen_ports = HashSet::new();

    if command_exists("ss") {
        if let Ok(raw) = run_command("ss", &["-tlnp"]) {
            for line in raw.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 6 {
                    continue;
                }
                let Some(port) = parse_port(parts[3]) else {
                    continue;
                };
                if seen_ports.contains(&port) {
                    continue;
                }
                let users = parts[5..].join(" ");
                let Some(pid) = extract_number_after(&users, "pid=") else {
                    continue;
                };
                let process_name = extract_quoted_process_name(&users)
                    .or_else(|| read_proc_comm(pid))
                    .unwrap_or_else(|| "unknown".into());
                seen_ports.insert(port);
                entries.push(ListeningEntry {
                    port,
                    pid,
                    process_name,
                });
            }
        }
    }

    if entries.is_empty() && command_exists("netstat") {
        let raw = run_command("netstat", &["-tlnp"])?;
        for line in raw.lines() {
            if !line.contains("LISTEN") {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 7 {
                continue;
            }
            let Some(port) = parse_port(parts[3]) else {
                continue;
            };
            if seen_ports.contains(&port) {
                continue;
            }
            let pid_program = parts[parts.len() - 1];
            let Some((pid, process_name)) = parse_pid_program(pid_program) else {
                continue;
            };
            seen_ports.insert(port);
            entries.push(ListeningEntry {
                port,
                pid,
                process_name,
            });
        }
    }

    Ok(entries)
}

#[cfg(target_os = "windows")]
fn get_listening_ports_raw_windows() -> Result<Vec<ListeningEntry>, String> {
    let raw = run_command("netstat", &["-ano", "-p", "TCP"])?;
    let mut entries = Vec::new();
    let mut seen_ports = HashSet::new();
    let mut pids = HashSet::new();

    for line in raw.lines().filter(|line| line.contains("LISTENING")) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let Some(port) = parse_port(parts[1]) else {
            continue;
        };
        if seen_ports.contains(&port) {
            continue;
        }
        let Ok(pid) = parts[parts.len() - 1].parse::<u32>() else {
            continue;
        };
        seen_ports.insert(port);
        pids.insert(pid);
        entries.push(ListeningEntry {
            port,
            pid,
            process_name: String::new(),
        });
    }

    let names = get_windows_process_names(pids.into_iter().collect())?;
    for entry in &mut entries {
        entry.process_name = names.get(&entry.pid).cloned().unwrap_or_else(|| "unknown".into());
    }

    Ok(entries)
}

fn batch_process_info(pids: &[u32]) -> Result<HashMap<u32, ProcessSnapshot>, String> {
    #[cfg(target_os = "macos")]
    {
        batch_process_info_unix("ps", pids)
    }
    #[cfg(target_os = "linux")]
    {
        batch_process_info_linux(pids)
    }
    #[cfg(target_os = "windows")]
    {
        batch_process_info_windows(pids)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn batch_process_info_unix(cmd: &str, pids: &[u32]) -> Result<HashMap<u32, ProcessSnapshot>, String> {
    let mut map = HashMap::new();
    if pids.is_empty() {
        return Ok(map);
    }

    let pid_list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let raw = run_command(
        cmd,
        &["-p", &pid_list, "-o", "pid=,ppid=,stat=,rss=,etime=,command="],
    )?;
    for line in raw.lines() {
        let Some(snapshot) = parse_ps_snapshot(line) else {
            continue;
        };
        map.insert(snapshot.0, snapshot.1);
    }

    Ok(map)
}

#[cfg(target_os = "linux")]
fn batch_process_info_linux(pids: &[u32]) -> Result<HashMap<u32, ProcessSnapshot>, String> {
    let mut map = batch_process_info_unix("ps", pids)?;

    for pid in pids {
        if map.contains_key(pid) {
            continue;
        }
        let proc_dir = PathBuf::from(format!("/proc/{pid}"));
        if !proc_dir.exists() {
            continue;
        }
        let stat = fs::read_to_string(proc_dir.join("stat")).unwrap_or_default();
        let statm = fs::read_to_string(proc_dir.join("statm")).unwrap_or_default();
        let command = fs::read(proc_dir.join("cmdline"))
            .map(|bytes| {
                bytes
                    .split(|byte| *byte == 0)
                    .filter(|part| !part.is_empty())
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let (ppid, stat_code) = parse_linux_proc_stat(&stat);
        let rss_pages = statm
            .split_whitespace()
            .nth(1)
            .and_then(|part| part.parse::<u64>().ok())
            .unwrap_or(0);

        map.insert(
            *pid,
            ProcessSnapshot {
                ppid,
                stat: stat_code,
                rss_kb: rss_pages * 4,
                elapsed_secs: None,
                command: if command.is_empty() {
                    read_proc_comm(*pid).unwrap_or_default()
                } else {
                    command
                },
            },
        );
    }

    Ok(map)
}

#[cfg(target_os = "windows")]
fn batch_process_info_windows(pids: &[u32]) -> Result<HashMap<u32, ProcessSnapshot>, String> {
    let mut map = HashMap::new();
    if pids.is_empty() {
        return Ok(map);
    }

    let condition = pids
        .iter()
        .map(|pid| format!("ProcessId={pid}"))
        .collect::<Vec<_>>()
        .join(" or ");
    let raw = run_command(
        "wmic",
        &[
            "process",
            "where",
            &format!("({condition})"),
            "get",
            "ProcessId,ParentProcessId,WorkingSetSize,CommandLine,Name",
            "/format:csv",
        ],
    )
    .or_else(|_| {
        run_command(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                &format!(
                    "Get-CimInstance Win32_Process | Where-Object {{{}}} | Select-Object ProcessId,ParentProcessId,WorkingSetSize,CommandLine,Name | ConvertTo-Csv -NoTypeInformation",
                    pids.iter()
                        .map(|pid| format!("$_.ProcessId -eq {pid}"))
                        .collect::<Vec<_>>()
                        .join(" -or ")
                ),
            ],
        )
    })?;

    for line in raw.lines().filter(|line| line.contains(',')) {
        let parts = parse_csv_line(line);
        if parts.len() < 6 {
            continue;
        }
        let pid = parts
            .iter()
            .rev()
            .find_map(|part| part.parse::<u32>().ok())
            .unwrap_or(0);
        if pid == 0 {
            continue;
        }
        let ppid = parts
            .iter()
            .find_map(|part| part.parse::<u32>().ok())
            .unwrap_or(0);
        let rss_bytes = parts
            .iter()
            .find_map(|part| part.parse::<u64>().ok())
            .unwrap_or(0);
        let command = parts
            .iter()
            .find(|part| part.contains('\\') || part.contains(' '))
            .cloned()
            .unwrap_or_default();
        map.insert(
            pid,
            ProcessSnapshot {
                ppid,
                stat: "S".into(),
                rss_kb: rss_bytes / 1024,
                elapsed_secs: None,
                command,
            },
        );
    }

    Ok(map)
}

fn batch_cwd(pids: &[u32]) -> Result<HashMap<u32, PathBuf>, String> {
    #[cfg(target_os = "macos")]
    {
        batch_cwd_macos(pids)
    }
    #[cfg(target_os = "linux")]
    {
        batch_cwd_linux(pids)
    }
    #[cfg(target_os = "windows")]
    {
        batch_cwd_windows(pids)
    }
}

#[cfg(target_os = "macos")]
fn batch_cwd_macos(pids: &[u32]) -> Result<HashMap<u32, PathBuf>, String> {
    let mut map = HashMap::new();
    if pids.is_empty() {
        return Ok(map);
    }
    let pid_list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let raw = run_command_allow_nonzero("lsof", &["-a", "-d", "cwd", "-p", &pid_list])?;
    for line in raw.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let Ok(pid) = parts[1].parse::<u32>() else {
            continue;
        };
        let path = parts[8..].join(" ");
        if path.starts_with('/') {
            map.insert(pid, PathBuf::from(path));
        }
    }
    Ok(map)
}

#[cfg(target_os = "linux")]
fn batch_cwd_linux(pids: &[u32]) -> Result<HashMap<u32, PathBuf>, String> {
    let mut map = HashMap::new();
    for pid in pids {
        let link = PathBuf::from(format!("/proc/{pid}/cwd"));
        if let Ok(path) = fs::read_link(&link) {
            if path.is_absolute() {
                map.insert(*pid, path);
            }
        }
    }
    Ok(map)
}

#[cfg(target_os = "windows")]
fn batch_cwd_windows(pids: &[u32]) -> Result<HashMap<u32, PathBuf>, String> {
    let mut map = HashMap::new();
    if pids.is_empty() {
        return Ok(map);
    }
    let condition = pids
        .iter()
        .map(|pid| format!("ProcessId={pid}"))
        .collect::<Vec<_>>()
        .join(" or ");
    let raw = run_command(
        "wmic",
        &[
            "process",
            "where",
            &format!("({condition})"),
            "get",
            "ProcessId,ExecutablePath",
            "/format:csv",
        ],
    )?;
    for line in raw.lines().filter(|line| line.contains(',')) {
        let parts = parse_csv_line(line);
        if parts.len() < 3 {
            continue;
        }
        let pid = parts.iter().find_map(|part| part.parse::<u32>().ok()).unwrap_or(0);
        let exe = parts
            .iter()
            .find(|part| part.contains('\\'))
            .map(PathBuf::from);
        if pid != 0 {
            if let Some(path) = exe.and_then(|path| path.parent().map(PathBuf::from)) {
                map.insert(pid, path);
            }
        }
    }
    Ok(map)
}

fn get_all_processes_raw() -> Result<Vec<ProcessListEntry>, String> {
    #[cfg(target_os = "macos")]
    {
        get_all_processes_raw_unix("ps")
    }
    #[cfg(target_os = "linux")]
    {
        get_all_processes_raw_unix("ps")
    }
    #[cfg(target_os = "windows")]
    {
        get_all_processes_raw_windows()
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn get_all_processes_raw_unix(cmd: &str) -> Result<Vec<ProcessListEntry>, String> {
    let raw = run_command(cmd, &["-eo", "pid=,pcpu=,rss=,etime=,command="])?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let current_pid = process::id();

    for line in raw.lines() {
        let Some((pid, cpu, rss_kb, etime, command)) = parse_ps_process_line(line) else {
            continue;
        };
        if pid <= 1 || pid == current_pid || !seen.insert(pid) {
            continue;
        }
        let process_name = command
            .split_whitespace()
            .next()
            .and_then(|part| Path::new(part).file_name().and_then(|name| name.to_str()))
            .unwrap_or("unknown")
            .to_string();
        entries.push(ProcessListEntry {
            pid,
            process_name,
            cpu,
            rss_kb,
            elapsed_secs: parse_elapsed(&etime),
            command,
        });
    }

    Ok(entries)
}

#[cfg(target_os = "windows")]
fn get_all_processes_raw_windows() -> Result<Vec<ProcessListEntry>, String> {
    let raw = run_command(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-Process | Select-Object Id,CPU,WorkingSet64,ProcessName,Path | ConvertTo-Csv -NoTypeInformation",
        ],
    )?;

    let mut entries = Vec::new();
    let current_pid = process::id();
    let mut seen = HashSet::new();

    for line in raw.lines().skip(1) {
        let parts = parse_csv_line(line);
        if parts.len() < 5 {
            continue;
        }
        let pid = parts[0].parse::<u32>().unwrap_or(0);
        if pid <= 4 || pid == current_pid || !seen.insert(pid) {
            continue;
        }
        let cpu = parts[1].parse::<f64>().unwrap_or(0.0);
        let rss_bytes = parts[2].parse::<u64>().unwrap_or(0);
        let process_name = parts[3].clone();
        let command = if parts[4].is_empty() {
            process_name.clone()
        } else {
            parts[4].clone()
        };

        entries.push(ProcessListEntry {
            pid,
            process_name,
            cpu,
            rss_kb: rss_bytes / 1024,
            elapsed_secs: None,
            command,
        });
    }

    Ok(entries)
}

fn get_process_tree(pid: u32) -> Result<Vec<ProcessTreeNode>, String> {
    #[cfg(target_os = "macos")]
    {
        get_process_tree_macos(pid)
    }
    #[cfg(target_os = "linux")]
    {
        get_process_tree_linux(pid)
    }
    #[cfg(target_os = "windows")]
    {
        get_process_tree_windows(pid)
    }
}

#[cfg(target_os = "macos")]
fn get_process_tree_macos(pid: u32) -> Result<Vec<ProcessTreeNode>, String> {
    let raw = run_command("ps", &["-eo", "pid=,ppid=,comm="])?;
    let mut processes = HashMap::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(entry_pid) = parts[0].parse::<u32>() else {
            continue;
        };
        let Ok(ppid) = parts[1].parse::<u32>() else {
            continue;
        };
        processes.insert(
            entry_pid,
            ProcessTreeNode {
                pid: entry_pid,
                ppid,
                name: parts[2..].join(" "),
            },
        );
    }
    build_process_tree(processes, pid)
}

#[cfg(target_os = "linux")]
fn get_process_tree_linux(pid: u32) -> Result<Vec<ProcessTreeNode>, String> {
    let mut processes = HashMap::new();
    for entry in fs::read_dir("/proc").map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(entry_pid) = name.parse::<u32>() else {
            continue;
        };
        let stat = fs::read_to_string(format!("/proc/{entry_pid}/stat")).unwrap_or_default();
        let (ppid, state) = parse_linux_proc_stat(&stat);
        let proc_name = extract_proc_name(&stat).unwrap_or(state);
        processes.insert(
            entry_pid,
            ProcessTreeNode {
                pid: entry_pid,
                ppid,
                name: proc_name,
            },
        );
    }
    build_process_tree(processes, pid)
}

#[cfg(target_os = "windows")]
fn get_process_tree_windows(pid: u32) -> Result<Vec<ProcessTreeNode>, String> {
    let raw = run_command("wmic", &["process", "get", "ProcessId,ParentProcessId,Name", "/format:csv"])?;
    let mut processes = HashMap::new();
    for line in raw.lines().filter(|line| line.contains(',')) {
        let parts = parse_csv_line(line);
        if parts.len() < 4 {
            continue;
        }
        let name = parts.get(1).cloned().unwrap_or_default();
        let ppid = parts.get(2).and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
        let entry_pid = parts.get(3).and_then(|part| part.parse::<u32>().ok()).unwrap_or(0);
        if entry_pid != 0 {
            processes.insert(
                entry_pid,
                ProcessTreeNode {
                    pid: entry_pid,
                    ppid,
                    name,
                },
            );
        }
    }
    build_process_tree(processes, pid)
}

fn build_process_tree(
    processes: HashMap<u32, ProcessTreeNode>,
    pid: u32,
) -> Result<Vec<ProcessTreeNode>, String> {
    let mut tree = Vec::new();
    let mut current = pid;
    let mut depth = 0;
    while current > 1 && depth < 8 {
        let Some(node) = processes.get(&current).cloned() else {
            break;
        };
        current = node.ppid;
        tree.push(node);
        depth += 1;
    }
    Ok(tree)
}

fn run_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;

    if !output.status.success() {
        return Err(format!("{program} exited with {}", output.status));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_command_allow_nonzero(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn command_exists(program: &str) -> bool {
    #[cfg(unix)]
    {
        Command::new("which")
            .arg(program)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        Command::new("where")
            .arg(program)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn parse_port(value: &str) -> Option<u16> {
    let trimmed = value.trim().trim_matches(|ch| ch == '[' || ch == ']');
    trimmed.rsplit(':').next()?.parse::<u16>().ok()
}

fn parse_ps_snapshot(line: &str) -> Option<(u32, ProcessSnapshot)> {
    let mut parts = line.trim_start().split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let ppid = parts.next()?.parse::<u32>().ok()?;
    let stat = parts.next()?.to_string();
    let rss_kb = parts.next()?.parse::<u64>().ok()?;
    let etime = parts.next()?.to_string();
    let command = parts.collect::<Vec<_>>().join(" ");
    Some((
        pid,
        ProcessSnapshot {
            ppid,
            stat,
            rss_kb,
            elapsed_secs: parse_elapsed(&etime),
            command,
        },
    ))
}

fn parse_ps_process_line(line: &str) -> Option<(u32, f64, u64, String, String)> {
    let mut parts = line.trim_start().split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let cpu = parts.next()?.parse::<f64>().ok()?;
    let rss_kb = parts.next()?.parse::<u64>().ok()?;
    let etime = parts.next()?.to_string();
    let command = parts.collect::<Vec<_>>().join(" ");
    Some((pid, cpu, rss_kb, etime, command))
}

fn parse_elapsed(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (days, remainder) = if let Some((day, rest)) = value.split_once('-') {
        (day.parse::<u64>().ok()?, rest)
    } else {
        (0, value)
    };
    let parts: Vec<&str> = remainder.split(':').collect();
    let secs = match parts.as_slice() {
        [minutes, seconds] => minutes.parse::<u64>().ok()? * 60 + seconds.parse::<u64>().ok()?,
        [hours, minutes, seconds] => {
            hours.parse::<u64>().ok()? * 3600
                + minutes.parse::<u64>().ok()? * 60
                + seconds.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

fn format_memory(rss_kb: u64) -> String {
    if rss_kb > 1_048_576 {
        format!("{:.1} GB", rss_kb as f64 / 1_048_576.0)
    } else if rss_kb > 1_024 {
        format!("{:.1} MB", rss_kb as f64 / 1_024.0)
    } else {
        format!("{rss_kb} KB")
    }
}

#[cfg(target_os = "linux")]
fn read_proc_comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|value| value.trim().to_string())
}

#[cfg(target_os = "linux")]
fn parse_linux_proc_stat(stat: &str) -> (u32, String) {
    let Some(end) = stat.rfind(')') else {
        return (0, String::new());
    };
    let tail: Vec<&str> = stat[end + 2..].split_whitespace().collect();
    let state = tail.first().copied().unwrap_or("").to_string();
    let ppid = tail
        .get(1)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    (ppid, state)
}

#[cfg(target_os = "linux")]
fn extract_proc_name(stat: &str) -> Option<String> {
    let start = stat.find('(')?;
    let end = stat.rfind(')')?;
    Some(stat[start + 1..end].to_string())
}

#[cfg(target_os = "linux")]
fn extract_number_after(value: &str, token: &str) -> Option<u32> {
    let start = value.find(token)? + token.len();
    let digits: String = value[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse::<u32>().ok()
}

#[cfg(target_os = "linux")]
fn extract_quoted_process_name(value: &str) -> Option<String> {
    let start = value.find("(\"")? + 2;
    let end = value[start..].find('"')?;
    Some(value[start..start + end].to_string())
}

#[cfg(target_os = "linux")]
fn parse_pid_program(value: &str) -> Option<(u32, String)> {
    let (pid, program) = value.split_once('/')?;
    Some((pid.parse::<u32>().ok()?, program.to_string()))
}

#[cfg(target_os = "windows")]
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

#[cfg(target_os = "windows")]
fn get_windows_process_names(pids: Vec<u32>) -> Result<HashMap<u32, String>, String> {
    let mut map = HashMap::new();
    if pids.is_empty() {
        return Ok(map);
    }
    let condition = pids
        .iter()
        .map(|pid| format!("ProcessId={pid}"))
        .collect::<Vec<_>>()
        .join(" or ");
    let raw = run_command(
        "wmic",
        &[
            "process",
            "where",
            &format!("({condition})"),
            "get",
            "ProcessId,Name",
            "/format:csv",
        ],
    )?;
    for line in raw.lines().filter(|line| line.contains(',')) {
        let parts = parse_csv_line(line);
        if parts.len() < 3 {
            continue;
        }
        let pid = parts[2].parse::<u32>().unwrap_or(0);
        if pid != 0 {
            map.insert(pid, parts[1].trim_end_matches(".exe").to_string());
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elapsed_values() {
        assert_eq!(parse_elapsed("03:10"), Some(190));
        assert_eq!(parse_elapsed("01:02:03"), Some(3723));
        assert_eq!(parse_elapsed("2-01:02:03"), Some(176_523));
    }

    #[test]
    fn formats_uptime_values() {
        assert_eq!(format_uptime(8), "8s");
        assert_eq!(format_uptime(130), "2m 10s");
        assert_eq!(format_uptime(9_000), "2h 30m");
    }

    #[test]
    fn detects_framework_from_package_text() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(detect_framework(&root), Some("Rust".into()));
    }
}
