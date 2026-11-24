//! Row iteration that runs in parallel when the `parallel` feature is on.

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Calls `work(row_index, row)` for every `stride`-sized row of `buffer`.
pub(crate) fn for_each_row<T, F>(buffer: &mut [T], stride: usize, work: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync + Send,
{
    if stride == 0 {
        return;
    }
    #[cfg(feature = "parallel")]
    buffer
        .par_chunks_mut(stride)
        .enumerate()
        .for_each(|(index, row)| work(index, row));
    #[cfg(not(feature = "parallel"))]
    buffer
        .chunks_mut(stride)
        .enumerate()
        .for_each(|(index, row)| work(index, row));
}

/// Builds a buffer of `rows` rows, `stride` samples each, filling every row
/// with `work(row_index, row)`.
pub(crate) fn build_rows<T, F>(rows: usize, stride: usize, work: F) -> Vec<T>
where
    T: Send + Default + Clone,
    F: Fn(usize, &mut [T]) + Sync + Send,
{
    let mut buffer = vec![T::default(); rows * stride];
    for_each_row(&mut buffer, stride, work);
    buffer
}
