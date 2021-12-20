pub trait Sealed {}

impl<S: Sealed> Sealed for &S {}
impl Sealed for () {}
