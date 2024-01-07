use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    address: String,
}

#[tokio::main]
pub async fn main() {
    let args = Args::parse();

    println!("starting signal {}", args.address);

    signal::server(&args.address).await.unwrap();
}
