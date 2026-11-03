use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(address) = std::env::args().nth(1) else {
        eprintln!("Usage: dragon-browser-headless <url>");
        return ExitCode::from(2);
    };

    let address = address.trim();
    let address = if address.contains("://") {
        address.to_owned()
    } else {
        format!("https://{address}")
    };

    match dragon_core::load_url(&address) {
        Ok(article) => {
            println!("URL: {address}");
            println!("Title: {}", article.title);
            println!("Blocks: {}", article.blocks.len());
            println!("Images: {}", article.images.len());
            println!();
            println!("{}", article.text);

            if !article.resource_warnings.is_empty() {
                eprintln!("\nWarnings:");
                for warning in article.resource_warnings {
                    eprintln!("- {warning}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Could not load {address}: {error}");
            ExitCode::FAILURE
        }
    }
}
