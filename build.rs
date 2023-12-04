use clap::CommandFactory;
use clap_complete::Shell;

include!("src/args.rs");

fn main() {
    println!("cargo:rerun-if-env-changed=COMPLETIONS_DIR");

    let directory = match std::env::var_os("COMPLETIONS_DIR") {
        None => return,
        Some(out_dir) => out_dir,
    };

    let mut app = Args::command();
    let name = app.get_name().to_string();

    clap_complete::generate_to(Shell::Bash, &mut app, &name, &directory).unwrap();
    clap_complete::generate_to(Shell::Fish, &mut app, &name, &directory).unwrap();
    clap_complete::generate_to(Shell::Zsh, &mut app, &name, &directory).unwrap();
}
