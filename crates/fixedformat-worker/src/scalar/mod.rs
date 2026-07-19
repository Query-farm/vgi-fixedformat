//! Scalar functions exposed by the fixedformat worker.

mod pack;
mod unpack;
mod unpack_multi;

use vgi::Worker;

/// Register every scalar function on the worker.
pub fn register(worker: &mut Worker) {
    // Two arity overloads each: (rec, spec) and (rec, spec, encoding). DuckDB
    // scalar functions take only positional args, so `encoding` is positional.
    worker.register_scalar(unpack::Unpack {
        with_encoding: false,
    });
    worker.register_scalar(unpack::Unpack {
        with_encoding: true,
    });
    worker.register_scalar(pack::Pack {
        with_encoding: false,
    });
    worker.register_scalar(pack::Pack {
        with_encoding: true,
    });
    worker.register_scalar(unpack_multi::UnpackMulti {
        with_encoding: false,
    });
    worker.register_scalar(unpack_multi::UnpackMulti {
        with_encoding: true,
    });
}
