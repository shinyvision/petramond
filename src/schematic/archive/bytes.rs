pub(super) fn uint(out: &mut Vec<u8>, mut n: usize) {
    while n >= 128 {
        out.push((n as u8) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
pub(super) fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    uint(out, bytes.len());
    out.extend_from_slice(bytes);
}
pub(super) struct Reader<R> {
    input: R,
}
impl<R: std::io::Read> Reader<R> {
    pub fn new(input: R) -> Self {
        Self { input }
    }
    pub fn take(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; n];
        self.input
            .read_exact(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
    pub fn byte(&mut self) -> Result<u8, String> {
        let mut byte = [0];
        self.input
            .read_exact(&mut byte)
            .map_err(|e| e.to_string())?;
        Ok(byte[0])
    }
    pub fn uint(&mut self, max: usize) -> Result<usize, String> {
        let mut n = 0usize;
        for shift in (0..usize::BITS).step_by(7) {
            let byte = self.byte()?;
            let value = usize::from(byte & 127);
            if value > (usize::MAX >> shift) {
                return Err("Invalid schematic integer".into());
            }
            n |= value << shift;
            if byte < 128 {
                if n > max || (shift > 0 && byte == 0) {
                    return Err("Invalid schematic integer".into());
                }
                return Ok(n);
            }
        }
        Err("Invalid schematic integer".into())
    }
    pub fn blob(&mut self, max: usize) -> Result<Vec<u8>, String> {
        let n = self.uint(max)?;
        self.take(n)
    }
    pub fn finish(&mut self) -> Result<(), String> {
        let mut byte = [0];
        if self.input.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            Ok(())
        } else {
            Err("Trailing schematic data".into())
        }
    }
}
