// FREEIMAGETOVIDEO_API_KEY=sk-... cargo run --example generate -- "a paper lantern drifting over a night river" [image_url]
fn main() {
    let mut args = std::env::args().skip(1);
    let prompt = args.next().unwrap_or_else(|| "a paper lantern drifting over a night river".into());
    let mut opts = freeimagetovideo::Options::new();
    if let Some(img) = args.next() {
        opts = opts.image(img);
    }
    match freeimagetovideo::generate_video(&prompt, &opts) {
        Ok(url) => println!("{url}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}
