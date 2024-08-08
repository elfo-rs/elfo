mod alice;
mod bob;
mod protocol;

#[tokio::main]
async fn main() {
    let topology = match std::env::args().nth(1).unwrap().as_str() {
        "alice" => alice::topology(),
        "bob" => bob::topology(),
        _ => unreachable!(),
    };

    elfo::init::start(topology).await;
}
