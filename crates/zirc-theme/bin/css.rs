//! Prints the web stylesheet, zirc-theme.css, generated from tokens.json.

fn main() {
    print!("{}", zirc_theme::css());
}
