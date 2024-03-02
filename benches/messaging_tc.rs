use tcmalloc::TCMalloc;

mod messaging;

#[global_allocator]
static ALLOCATOR: TCMalloc = TCMalloc;

criterion::criterion_main!(messaging::cases);
