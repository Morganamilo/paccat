use clap::{App, IntoApp};
use clap_generate::{generate_to, Shell};

include!("src/args.rs");

fn main() {
    println!("cargo:rerun-if-env-changed=COMPLETIONS_DIR");

    let directory = match std::env::var_os("COMPLETIONS_DIR") {
        None => return,
        Some(out_dir) => out_dir,
    };

    let mut app: App = Args::into_app();

    generate_to(Shell::Bash, &mut app, env!("CARGO_PKG_NAME"), &directory).unwrap();
    generate_to(Shell::Fish, &mut app, env!("CARGO_PKG_NAME"), &directory).unwrap();
    generate_to(Shell::Zsh, &mut app, env!("CARGO_PKG_NAME"), &directory).unwrap();
}
