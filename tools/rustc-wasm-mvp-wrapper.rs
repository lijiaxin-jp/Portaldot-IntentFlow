use std::{
    env,
    ffi::OsString,
    process::{self, Command},
};

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(compiler) = args.next() else {
        eprintln!("rustc wrapper received no compiler path");
        process::exit(2);
    };
    let rustc_args: Vec<OsString> = args.collect();
    let is_wasm_target = rustc_args.windows(2).any(|pair| {
        pair[0] == "--target" && pair[1] == "wasm32-unknown-unknown"
    }) || rustc_args.iter().any(|arg| arg == "--target=wasm32-unknown-unknown");

    let mut command = Command::new(compiler);
    command.args(&rustc_args);
    if is_wasm_target {
        command
            .arg("-C")
            .arg("target-feature=-bulk-memory,-nontrapping-fptoint");
    }

    match command.status() {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("failed to launch rustc: {error}");
            process::exit(1);
        }
    }
}
