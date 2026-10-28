fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let operation = args.next().ok_or("expected write or read")?;
    let token = args.next().ok_or("expected synthetic token")?;
    if !token.starts_with("glyphfolio-clipboard-") || token.len() > 100 {
        return Err("probe accepts only bounded synthetic test tokens".into());
    }
    let text = format!("**{token}**\r\n日本 é 👩‍👩‍👧‍👦\n");
    let mut clipboard = arboard::Clipboard::new()?;
    match operation.as_str() {
        "write" => {
            clipboard.set_text(text)?;
            println!("WRITE_OK");
        }
        "read" => {
            if clipboard.get_text()? != text {
                return Err("synthetic clipboard readback mismatch".into());
            }
            println!("READ_OK");
        }
        _ => return Err("expected write or read".into()),
    }
    Ok(())
}
