mod backend;
mod core;

#[tokio::main]
async fn main() {
    let topology = match std::env::args().nth(1).unwrap().as_str() {
        "core" => core::topology(),
        "backend" => backend::topology(),
        _ => unreachable!(),
    };

    elfo::start(topology).await;
}
