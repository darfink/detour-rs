/// Functionality for generating PIC.
pub struct Generator {
    thunks: Vec<Box<Thunkable>>,
}

impl Generator {
    /// Constructs a new PIC generator.
    pub fn new() -> Self {
        Generator { thunks: Vec::new() }
    }

    /// Generates code for use at the specified address.
    pub fn generate(&self, base: *const ()) -> Vec<u8> {
        let mut result = Vec::with_capacity(self.len());
        let mut base = base as usize;

        for thunk in &self.thunks {
            // Retrieve the code for the segment
            let code = thunk.generate(base);
            assert_eq!(code.len(), thunk.len());

            // Advance the current EIP address
            base += thunk.len();
            result.extend(code);
        }

        result
    }

    /// Adds a position-independant code segment.
    pub fn add_thunk(&mut self, thunk: Box<Thunkable>) {
        self.thunks.push(thunk);
    }

    /// Returns the total size of a all code segments.
    pub fn len(&self) -> usize {
        self.thunks.iter().fold(0, |sum, thunk| sum + thunk.len())
    }
}

/// An interface for generating PIC thunks.
pub trait Thunkable {
    /// Generates the code at the specified address.
    fn generate(&self, address: usize) -> Vec<u8>;

    /// Returns the size of a generated thunk.
    fn len(&self) -> usize;
}

/// Thunkable implementation for static data
impl Thunkable for Vec<u8> {
    /// Generates static thunks assumed to be PIC
    fn generate(&self, _address: usize) -> Vec<u8> {
        self.clone()
    }

    /// Returns the size of a generated thunk
    fn len(&self) -> usize {
        self.len()
    }
}
