use directories::UserDirs;
use std::fs;

pub fn run() {
    let user_dirs = UserDirs::new().expect("could not determine home directory");
    let config_dir = user_dirs.home_dir().join(".hotdata");
    let config_file = config_dir.join("config.yml");

    if config_file.exists() {
        eprintln!("config file already exists at {}", config_file.display());
        std::process::exit(1);
    }

    if let Err(e) = fs::create_dir_all(&config_dir) {
        eprintln!("error creating config directory: {e}");
        std::process::exit(1);
    }

    let content = "profiles:\n  default:\n    api_key: PLACEHOLDER\n";

    if let Err(e) = fs::write(&config_file, content) {
        eprintln!("error writing config file: {e}");
        std::process::exit(1);
    }

    println!("created config file at {}", config_file.display());
}
