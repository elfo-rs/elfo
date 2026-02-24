//! Shows how to connect distributed actor groups.
//!
//! For simplicity, it uses one common binary that runs a specific service based
//! on the CLI argument:
//! ```sh
//! cargo run --bin network --features network -- alice &
//! cargo run --bin network --features network -- bob
//! ```

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
