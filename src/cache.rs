#[derive(Default)]
pub struct CacheProcess<In: Eq, Out> {
    previous: Option<(In, Out)>,
}

impl<In: Eq, Out> CacheProcess<In, Out> {
    pub fn process(&mut self, input: In, func: impl FnOnce(&In) -> Out) -> &Out {
        let old_cache = self.previous.take();

        let new_cache = match old_cache {
            Some((prev_in, prev_out)) if prev_in == input => (prev_in, prev_out),
            _ => {
                let new_out = func(&input);
                (input, new_out)
            }
        };

        &self.previous.insert(new_cache).1
    }
}
