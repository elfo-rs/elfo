use mimalloc::MiMalloc;

mod messaging;

#[global_allocator]
static ALLOCATOR: MiMalloc = MiMalloc;

criterion::criterion_main!(messaging::cases);
