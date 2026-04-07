use std::alloc::System;

mod mailbox;

#[global_allocator]
static ALLOCATOR: System = System;

criterion::criterion_main!(mailbox::cases);
