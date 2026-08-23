//! Gateway entry point: MCP over stdio.
//!
//! # Where the token comes from
//!
//! The agent runtime launches this shim and hands it the agent's token, by
//! default through `AGENTBED_TOKEN`. The gateway relays that token to the
//! broker on every call and never inspects it: it holds a credential in
//! transit, not a verifier, and cannot tell a valid token from an invalid one.
//! Only the broker can.
//!
//! Refusing to start without a token is deliberate. A gateway that starts
//! anyway would produce `unauthenticated` for every call and look like a broker
//! fault; failing here names the real problem.

use agentbed_gw::{BrokerClient, Session};
use agentbed_protocol::wire::Token;
use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: agentbed-gw --socket <broker socket> [--token-file <path>]\n\
     the agent's token is read from AGENTBED_TOKEN unless --token-file is given";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("agentbed-gw: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut socket_path: Option<PathBuf> = None;
    let mut token_file: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket_path = Some(PathBuf::from(next_value(&mut args, &arg)?)),
            "--token-file" => token_file = Some(PathBuf::from(next_value(&mut args, &arg)?)),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }

    let socket_path = socket_path.ok_or_else(|| format!("--socket is required\n\n{USAGE}"))?;
    let token = load_token(token_file.as_deref())?;

    let mut session = Session::new(BrokerClient::new(socket_path), token);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin: {e}"))?;
        if let Some(reply) = agentbed_gw::mcp::handle_line(&mut session, &line) {
            writeln!(stdout, "{reply}").map_err(|e| format!("stdout: {e}"))?;
            stdout.flush().map_err(|e| format!("stdout: {e}"))?;
        }
    }
    Ok(())
}

fn load_token(token_file: Option<&std::path::Path>) -> Result<Token, String> {
    if let Some(path) = token_file {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read token file {}: {e}", path.display()))?;
        return Ok(Token::new(raw.trim()));
    }
    // Removed from the environment once read, so it does not leak into any
    // child process the gateway might later spawn.
    let raw = std::env::var("AGENTBED_TOKEN")
        .map_err(|_| format!("no agent token: set AGENTBED_TOKEN or --token-file\n\n{USAGE}"))?;
    std::env::remove_var("AGENTBED_TOKEN");
    if raw.trim().is_empty() {
        return Err("AGENTBED_TOKEN is empty".to_owned());
    }
    Ok(Token::new(raw.trim()))
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}
