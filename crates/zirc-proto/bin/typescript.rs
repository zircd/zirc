//! Prints the web app's TypeScript types for the `zirc login` handshake. See
//! [`zirc_proto::login::typescript`].

fn main() {
    print!("{}", zirc_proto::login::typescript());
}
