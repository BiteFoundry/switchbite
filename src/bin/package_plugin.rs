use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [binary_flag, binary, output_flag, output]
            if binary_flag == "--binary" && output_flag == "--output" =>
        {
            switchbite::package::build(Path::new(binary), Path::new(output))
        }
        _ => Err("Usage: package-plugin --binary FILE --output FILE.zip".into()),
    };
    match result {
        Ok(report) => println!("{report}"),
        Err(e) => {
            eprintln!("Package failed: {e}");
            std::process::exit(1);
        }
    }
}
