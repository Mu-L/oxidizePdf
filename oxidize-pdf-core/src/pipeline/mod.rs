pub(crate) mod chunk_metadata;
pub mod element;
pub mod export;
pub mod graph;
pub mod hybrid_chunking;
pub mod partition;
pub mod profile;
pub mod rag;
pub mod reading_order;
pub mod semantic_chunking;
#[cfg(feature = "unstable-spi")]
pub mod spi;
pub mod token_counter;

#[cfg(feature = "language-detection")]
pub use chunk_metadata::detect_language;
pub use chunk_metadata::{ChunkMetadata, ContentTypeFlags, DocumentSource, PageRegion};
pub use element::{
    element_reading_order, Element, ElementBBox, ElementData, ElementMetadata, ImageElementData,
    KeyValueElementData, RichCell, TableElementData, TableStructure,
};
pub use export::{ElementMarkdownExporter, ExportConfig};
pub use graph::ElementGraph;
pub use hybrid_chunking::{
    ContextFormat, ContextMode, HybridChunk, HybridChunkConfig, HybridChunker, MergePolicy,
};
pub use partition::{PartitionConfig, Partitioner, ReadingOrderStrategy};
pub use profile::{ExtractionProfile, ProfileConfig};
pub use rag::RagChunk;
pub use reading_order::{ReadingOrder, SimpleReadingOrder, XYCutReadingOrder};
pub use semantic_chunking::{SemanticChunk, SemanticChunkConfig, SemanticChunker};
#[cfg(feature = "unstable-spi")]
pub use spi::{
    AnalysisPipeline, ChunkGroup, ChunkingStrategy, ClassLabel, ClassifyContext, ElementClassifier,
};
#[cfg(all(feature = "unstable-spi", feature = "semantic"))]
pub use spi::{EnrichContext, MetadataEnricher};
#[cfg(feature = "tiktoken")]
pub use token_counter::TiktokenCounter;
pub use token_counter::{TokenCounter, WordProxyCounter};
