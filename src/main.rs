use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(url) = args.next() else {
        eprintln!("Cách dùng: dragon-browser <URL>");
        return ExitCode::from(2);
    };

    match dragon_core::load_url(&url) {
        Ok(article) => {
            if !article.title.is_empty() {
                println!("{}\n", article.title);
            }
            println!("{}", article.text);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Không thể tải trang: {error}");
            ExitCode::FAILURE
        }
    }
}
