use jemallocator::Jemalloc;

mod messaging;

#[global_allocator]
static ALLOCATOR: Jemalloc = Jemalloc;

criterion::criterion_main!(messaging::cases);
