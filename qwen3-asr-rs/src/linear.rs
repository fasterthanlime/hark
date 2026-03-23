use candle_core::quantized::QMatMul;
use candle_core::{Result, Tensor};
use candle_nn::{Linear, Module};

/// Linear layer that supports both dense (safetensors) and quantized (GGUF)
/// weights behind a single `forward()` interface.
pub(crate) enum LinearW {
    /// Standard dense linear layer (weight + optional bias).
    Dense(Linear),
    /// Quantized matmul (GGUF Q4_K, Q8_0, etc.) + optional dequantized bias.
    Quant {
        qmatmul: QMatMul,
        bias: Option<Tensor>,
    },
}

impl LinearW {
    /// Create a dense linear layer (safetensors path).
    pub fn new(weight: Tensor, bias: Option<Tensor>) -> Self {
        Self::Dense(Linear::new(weight, bias))
    }

    /// Create a quantized linear layer from a QMatMul and optional bias tensor.
    pub fn from_qmatmul(qmatmul: QMatMul, bias: Option<Tensor>) -> Self {
        Self::Quant { qmatmul, bias }
    }

    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        match self {
            Self::Dense(linear) => linear.forward(x),
            Self::Quant { qmatmul, bias } => {
                let out = qmatmul.forward(x)?;
                match bias {
                    Some(b) => out.broadcast_add(b),
                    None => Ok(out),
                }
            }
        }
    }
}
