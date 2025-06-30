use reqwest::{blocking::multipart, header::AUTHORIZATION};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::{stdin, Read, Write},
    path::PathBuf,
    process::Command,
};
use zip::write::SimpleFileOptions;

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    build_command: Option<String>,
    directory: Option<String>,
    domain: Option<String>,
    auth: Option<String>,
}

impl Config {
    pub fn default() -> Config {
        Config {
            build_command: Some("npm run build".into()),
            directory: Some("build".into()),
            domain: None,
            auth: None,
        }
    }

    fn to_string(&self) -> String {
        let not_set = String::from("not set");
        return format!(
            "\tDomain: {},\n\tOutput Directory: {},\n\tBuild Command: {}\n\n",
            self.domain.clone().unwrap_or(not_set.clone()),
            self.directory.clone().unwrap_or(not_set.clone()),
            self.build_command.clone().unwrap_or(not_set)
        );
    }
}

fn main() -> () {
    let path = env::current_dir().expect("CURRENT PATH NOT SET");
    let config_path = path.join(".uploader");

    let mut config = Config::default();

    let is_default = match config_path.exists() {
        true => {
            let contents =
                fs::read_to_string(&config_path).expect("SHOULD HAVE BEEN ABLE TO READ THE FILE");

            config = serde_json::from_str(&contents).expect("FAILED TO DESERIALIZE CONFIG FILE");
            println!("\nCONFIG:\n{}", config.to_string());
            false
        }
        false => true,
    };

    if config.directory.is_none() || is_default {
        config.directory = read_from_stdin(String::from("SET THE DIRECTORY:"), config.directory);
    }

    if config.build_command.is_none() || is_default {
        config.build_command =
            read_from_stdin(String::from("SET THE BUILD COMMAND:"), config.build_command);
    }

    if config.domain.is_none() || is_default {
        config.domain = read_from_stdin(String::from("SET THE DOMAIN"), config.domain);
    }

    if let Some(domain) = &config.domain {
        if !domain.starts_with("http") {
            config.domain = Some(format!("https://{}", domain));
        }
    }

    if config.auth.is_none() || is_default {
        config.auth = read_from_stdin(String::from("AUTHENTICATION KEY"), config.auth);
    }

    let result = run_build(&config);

    if result.is_err() {
        println!("build failed, exiting");
        return;
    }

    let zip = zip_output(&path, &config);

    upload_zip(zip, &config);

    // write config file
    let serialized = serde_json::to_string_pretty(&config).expect("CONFIG COULD NOT BE SERIALIZED");
    fs::write(config_path, serialized).expect("CONFIG COULD NOT BE WRITTEN");

    // if its a git repo, add the config file to the .gitignore
    let gitignore = path.join(".gitignore");

    if gitignore.exists() {
        let contents = fs::read_to_string(&gitignore).expect("GITIGNORE COULD NOT BE READ");
        let mut lines = contents.lines().collect::<Vec<&str>>();

        if !lines.contains(&".uploader") {
            lines.push(".uploader");
            let new_contents = lines.join("\n");
            fs::write(gitignore, new_contents).expect("GITIGNORE COULD NOT BE WRITTEN");
        }
    }
}

fn read_from_stdin(query: String, default: Option<String>) -> Option<String> {
    let mut buffer = String::new();

    let mut res: Option<String> = None;

    while res.is_none() {
        println!("{}", query);

        if let Some(value) = &default {
            println!("\tdefault: {}", value);
        }

        stdin().read_line(&mut buffer).expect("ERROR READING LINE");
        res = match buffer.trim() {
            "" => default.clone(),
            result => Some(result.to_string()),
        }
    }

    res
}

fn run_build(config: &Config) -> Result<(), ()> {
    if let Some(command) = &config.build_command {
        println!("RUNNING BUILD COMMAND: {}", command);

        let command_result = {
            #[cfg(target_os = "windows")]
            {
                Command::new("cmd").args(&["/C", command]).status()
            }

            #[cfg(not(target_os = "windows"))]
            {
                // On Unix-like systems, use 'sh -c'
                Command::new("sh").arg("-c").arg(command).status()
            }
        };
        println!("\n\n");

        return match command_result {
            Ok(status) => {
                if status.success() {
                    println!("BUILD SUCCESSFUL\n\n");
                    Ok(())
                } else {
                    println!("BUILD FAILED: {}\n\n", status);
                    Err(())
                }
            }
            Err(err) => {
                println!("ERROR RUNNING BUILD COMMAND: {}", err);
                Err(())
            }
        };
    }

    println!("NO BUILD COMMAND SET");
    return Ok(());
}

fn zip_output<'a>(base_path: &PathBuf, config: &Config) -> PathBuf {
    let dir = config.directory.to_owned().expect("output not set");
    let path = base_path.join(&dir);

    let exists = path.exists();
    println!("ZIPPING OUTPUT DIRECTORY: {}", path.to_string_lossy());

    if !exists {
        panic!("OUTPUT DIRECTORY DOES NOT EXIST");
    }

    // zip the output directory
    let output_path = base_path.join("output.zip");

    let output = fs::File::create(&output_path).expect("COULD NOT CREATE FILE");

    let mut zip = zip::ZipWriter::new(&output);

    let walk = walkdir::WalkDir::new(&path);

    let mut buffer = Vec::new();
    let dir_with_slash = format!("{}/", dir);

    for entry in walk.into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path
            .strip_prefix(&base_path)
            .expect("COULD NOT STRIP PREFIX");

        if path.is_file() {
            let mut name = name.to_string_lossy().to_string().replace("\\", "/");

            if name.starts_with(&dir_with_slash) {
                name = name.replacen(&dir_with_slash, "", 1);
            }

            zip.start_file(&name, SimpleFileOptions::default())
                .expect("COULD NOT START FILE");

            let mut file = fs::File::open(path).expect("COULD NOT OPEN FILE");
            file.read_to_end(&mut buffer).expect("COULD NOT READ FILE");

            zip.write(&buffer).expect("COULD NOT WRITE FILE");

            buffer.clear();
        }
    }

    zip.finish().expect("COULD NOT FINISH ZIP");
    println!("ZIP CREATED: {}", output_path.to_string_lossy());

    output_path
}

fn upload_zip(zip: PathBuf, config: &Config) {
    let domain = &config.domain.to_owned().expect("DOMAIN NOT SET");
    let form = multipart::Form::new()
        .file("zip", &zip)
        .expect("FROM COULD NOT BE CREATED");

    let auth = config.auth.to_owned().expect("AUTH NOT SET");

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(domain)
        .header(AUTHORIZATION, auth)
        .multipart(form)
        .send();

    match resp {
        Ok(response) => {
            if response.status().is_success() {
                println!("UPLOAD SUCCESSFUL");
            } else {
                println!("UPLOAD FAILED: {}", response.status());
            }
        }
        Err(err) => println!("ERROR UPLOADING, {}", err),
    };

    if let Err(err) = fs::remove_file(zip) {
        println!("ERROR REMOVING ZIP FILE: {}", err);
    }
}
