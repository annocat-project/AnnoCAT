pub mod normalization;
pub mod source_catalog;
pub mod vcf;

#[derive(Debug, Clone, Copy)]
pub struct AnnotationSource {
    pub id: &'static str,
    pub name: &'static str,
    pub purpose: &'static str,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SourceImplementation {
    pub source_id: &'static str,
    pub fastvep_source: Option<&'static str>,
    pub delivery: &'static str,
    pub assembly: &'static str,
}

pub const SOURCES: &[AnnotationSource] = &[
    AnnotationSource {
        id: "fastvep",
        name: "fastVEP",
        purpose: "Gene, transcript, consequence, HGVS, and regulatory annotation",
        default_enabled: true,
    },
    AnnotationSource {
        id: "dbnsfp",
        name: "dbNSFP 4.9a",
        purpose: "Missense/deleteriousness predictors, conservation, frequencies, and compiled functional fields",
        default_enabled: true,
    },
    AnnotationSource {
        id: "clinvar",
        name: "ClinVar",
        purpose: "Clinical significance, review status, conditions, and accessions",
        default_enabled: true,
    },
    AnnotationSource {
        id: "dbsnp",
        name: "dbSNP",
        purpose: "Reference SNP identifiers, merged-ID context, and per-allele frequency metadata",
        default_enabled: false,
    },
    AnnotationSource {
        id: "clingen",
        name: "ClinGen",
        purpose: "Dosage sensitivity and expert-curated gene/variant evidence",
        default_enabled: true,
    },
    AnnotationSource {
        id: "gencc",
        name: "GenCC",
        purpose: "Gene-disease validity and inheritance-mode evidence",
        default_enabled: true,
    },
    AnnotationSource {
        id: "gnomad",
        name: "gnomAD exomes",
        purpose: "Current exome population frequencies and ancestry-specific frequency context",
        default_enabled: false,
    },
    AnnotationSource {
        id: "gnomad-genomes",
        name: "gnomAD genomes",
        purpose: "Full genome-wide population frequencies, including non-coding regions",
        default_enabled: false,
    },
    AnnotationSource {
        id: "spliceai",
        name: "SpliceAI",
        purpose: "Ensembl MANE Select v1.4 masked splice-effect scores for GRCh38 SNVs",
        default_enabled: false,
    },
    AnnotationSource {
        id: "cadd",
        name: "CADD",
        purpose: "Genome-wide raw and PHRED-scaled deleteriousness scores",
        default_enabled: false,
    },
    AnnotationSource {
        id: "phylop",
        name: "PhyloP",
        purpose: "Genome-wide base-level evolutionary conservation scores",
        default_enabled: false,
    },
    AnnotationSource {
        id: "gerp",
        name: "GERP",
        purpose: "Dedicated genome-wide evolutionary constraint scores",
        default_enabled: false,
    },
    AnnotationSource {
        id: "revel",
        name: "REVEL",
        purpose: "Missense pathogenicity scores from a dedicated pinned release",
        default_enabled: false,
    },
    AnnotationSource {
        id: "primateai",
        name: "PrimateAI",
        purpose: "Dedicated primate-informed missense pathogenicity scores",
        default_enabled: false,
    },
    AnnotationSource {
        id: "dann",
        name: "DANN",
        purpose: "Dedicated genome-wide deleteriousness scores",
        default_enabled: false,
    },
    AnnotationSource {
        id: "alphamissense",
        name: "AlphaMissense",
        purpose: "Protein missense pathogenicity prediction",
        default_enabled: false,
    },
    AnnotationSource {
        id: "gnomad-constraint",
        name: "gnomAD gene constraint",
        purpose: "Gene-level loss-of-function and missense constraint metrics",
        default_enabled: false,
    },
    AnnotationSource {
        id: "omim",
        name: "OMIM",
        purpose: "Licensed user-supplied gene-disease relationships",
        default_enabled: false,
    },
    AnnotationSource {
        id: "cosmic",
        name: "COSMIC",
        purpose: "Licensed user-supplied somatic variant evidence",
        default_enabled: false,
    },
];

pub const SOURCE_IMPLEMENTATIONS: &[SourceImplementation] = &[
    SourceImplementation {
        source_id: "fastvep",
        fastvep_source: None,
        delivery: "bundled-engine",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "dbnsfp",
        fastvep_source: Some("dbnsfp"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "clinvar",
        fastvep_source: Some("clinvar"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "dbsnp",
        fastvep_source: Some("dbsnp"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "clingen",
        fastvep_source: None,
        delivery: "adapter-required",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "gencc",
        fastvep_source: None,
        delivery: "adapter-required",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "gnomad",
        fastvep_source: Some("gnomad"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "gnomad-genomes",
        fastvep_source: Some("gnomad"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "spliceai",
        fastvep_source: Some("spliceai"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "cadd",
        fastvep_source: Some("cadd"),
        delivery: "managed-public-noncommercial",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "phylop",
        fastvep_source: Some("phylop"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "gerp",
        fastvep_source: Some("gerp"),
        delivery: "catalog-pending",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "revel",
        fastvep_source: Some("revel"),
        delivery: "managed-public",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "primateai",
        fastvep_source: Some("primateai"),
        delivery: "catalog-pending",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "dann",
        fastvep_source: Some("dann"),
        delivery: "catalog-pending",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "alphamissense",
        fastvep_source: None,
        delivery: "adapter-required",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "gnomad-constraint",
        fastvep_source: Some("gnomad_genes"),
        delivery: "catalog-pending",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "omim",
        fastvep_source: Some("omim"),
        delivery: "user-supplied-licensed",
        assembly: "GRCh38",
    },
    SourceImplementation {
        source_id: "cosmic",
        fastvep_source: Some("cosmic"),
        delivery: "user-supplied-licensed",
        assembly: "GRCh38",
    },
];

pub fn source_implementation(source_id: &str) -> Option<&'static SourceImplementation> {
    SOURCE_IMPLEMENTATIONS
        .iter()
        .find(|implementation| implementation.source_id == source_id)
}

#[derive(Debug, Clone, Copy)]
pub struct AnnotationProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub purpose: &'static str,
    pub source_ids: &'static [&'static str],
}

const MINIMAL_SOURCE_IDS: &[&str] = &["clinvar", "dbsnp", "gnomad", "phylop", "revel"];
const COMPREHENSIVE_SOURCE_IDS: &[&str] = &[
    "dbnsfp", "clinvar", "dbsnp", "gnomad", "cadd", "phylop", "spliceai",
];

pub const ANNOTATION_PROFILES: &[AnnotationProfile] = &[
    AnnotationProfile {
        id: "wgs",
        name: "Comprehensive",
        purpose: "Broad clinical, population, prediction, conservation, and splicing annotation",
        source_ids: COMPREHENSIVE_SOURCE_IDS,
    },
    AnnotationProfile {
        id: "standard",
        name: "Minimal",
        purpose: "A smaller clinical and population set with dbSNP, conservation, and standalone REVEL scores",
        source_ids: MINIMAL_SOURCE_IDS,
    },
];

#[derive(Debug, Clone, Copy)]
pub struct ResourceRelease {
    pub resource_id: &'static str,
    pub version: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub download_bytes: Option<u64>,
    pub installed_bytes: Option<u64>,
    pub range_resume: bool,
    pub size_checked_at: &'static str,
    pub archive_format: &'static str,
    pub publisher_md5: Option<&'static str>,
    pub publisher_sha256: Option<&'static str>,
}

pub const RESOURCE_RELEASES: &[ResourceRelease] = &[
    ResourceRelease {
        resource_id: "dbnsfp",
        version: "4.9a",
        filename: "dbNSFP4.9a.zip",
        url: "https://usf.box.com/shared/static/0tq7q3b8ucaxxkmfyvnb0ss7g58ptgcl",
        download_bytes: Some(38_969_753_349),
        installed_bytes: None,
        range_resume: true,
        size_checked_at: "2026-07-14T23:31:59-04:00",
        archive_format: "zip",
        publisher_md5: Some("be89346ab3dc5c14a8a7b602f50c66fb"),
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "grch38-reference",
        version: "GCA_000001405.15",
        filename: "GRCh38_no_alt_analysis_set.fna.gz",
        url: "https://ftp.ncbi.nlm.nih.gov/genomes/all/GCA/000/001/405/GCA_000001405.15_GRCh38/seqs_for_alignment_pipelines.ucsc_ids/GCA_000001405.15_GRCh38_no_alt_analysis_set.fna.gz",
        download_bytes: Some(872_949_833),
        installed_bytes: None,
        range_resume: true,
        size_checked_at: "2026-07-15T02:15:09-04:00",
        archive_format: "gzip",
        publisher_md5: None,
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "ensembl-gff3",
        version: "115",
        filename: "Homo_sapiens.GRCh38.115.gff3.gz",
        url: "https://ftp.ensembl.org/pub/release-115/gff3/homo_sapiens/Homo_sapiens.GRCh38.115.gff3.gz",
        download_bytes: Some(83_835_106),
        installed_bytes: None,
        range_resume: true,
        size_checked_at: "2026-07-15T20:28:19-04:00",
        archive_format: "gzip",
        publisher_md5: None,
        publisher_sha256: Some("1e553efa8496d662e7264061a5cecf3001eb9a1157aaa66d80cd7ac35841509c"),
    },
    ResourceRelease {
        resource_id: "clinvar",
        version: "20260706",
        filename: "clinvar_20260706.vcf.gz",
        url: "https://ftp.ncbi.nlm.nih.gov/pub/clinvar/vcf_GRCh38/archive_2.0/2026/clinvar_20260706.vcf.gz",
        download_bytes: Some(192_290_992),
        installed_bytes: None,
        range_resume: true,
        size_checked_at: "2026-07-15T20:28:20-04:00",
        archive_format: "bgzip",
        publisher_md5: Some("f78d25d49e17a070957a127e409f87b9"),
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "dbsnp",
        version: "b157-GRCh38.p14",
        filename: "GCF_000001405.40.gz",
        url: "https://ftp.ncbi.nlm.nih.gov/snp/latest_release/VCF/GCF_000001405.40.gz",
        download_bytes: Some(29_552_227_779),
        installed_bytes: None,
        range_resume: true,
        size_checked_at: "2026-07-16T00:00:00-04:00",
        archive_format: "bgzip",
        publisher_md5: Some("6a6f313e92a39c337571174dad12cfe1"),
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "gnomad",
        version: "4.1.1-exomes",
        filename: "gnomad-v4.1.1-exomes.chromosome-streams",
        url: "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/exomes/",
        download_bytes: Some(199_241_266_182),
        installed_bytes: None,
        range_resume: false,
        size_checked_at: "2026-07-17T00:46:00-04:00",
        archive_format: "bgzip-shards",
        publisher_md5: None,
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "gnomad-genomes",
        version: "4.1.1-genomes",
        filename: "gnomad-v4.1.1-genomes.chromosome-streams",
        url: "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/genomes/",
        download_bytes: Some(565_643_483_329),
        installed_bytes: None,
        range_resume: false,
        size_checked_at: "2026-07-17T02:15:00-04:00",
        archive_format: "bgzip-shards",
        publisher_md5: None,
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "phylop",
        version: "hg38-100way-2015-05-08",
        filename: "hg38-phyloP100way.chromosome-streams",
        url: "https://hgdownload.soe.ucsc.edu/goldenPath/hg38/phyloP100way/hg38.100way.phyloP100way/",
        download_bytes: Some(5_452_453_066),
        installed_bytes: None,
        range_resume: false,
        size_checked_at: "2026-07-16T15:00:00-04:00",
        archive_format: "gzip-shards",
        publisher_md5: None,
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "cadd",
        version: "1.7",
        filename: "CADD-v1.7-GRCh38.chromosome-streams",
        url: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/",
        download_bytes: Some(88_735_216_521),
        installed_bytes: None,
        range_resume: false,
        size_checked_at: "2026-07-16T15:30:00-04:00",
        archive_format: "tabix-bgzip-ranges",
        publisher_md5: None,
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "spliceai",
        version: "ensembl-mane-v1.4-masked-snv",
        filename: "spliceai_scores.masked.snv.ensembl_mane_v1.4.grch38.chromosome-streams",
        url: "https://ftp.ensembl.org/pub/data_files/homo_sapiens/GRCh38/variation_plugins/spliceai_scores.masked.snv.ensembl_mane_v1.4.grch38.vcf.gz",
        download_bytes: Some(28_643_031_420),
        installed_bytes: None,
        range_resume: false,
        size_checked_at: "2026-07-16T17:19:31-04:00",
        archive_format: "tabix-bgzip-ranges",
        publisher_md5: None,
        publisher_sha256: None,
    },
    ResourceRelease {
        resource_id: "revel",
        version: "1.3",
        filename: "revel-v1.3.chromosome-zip-streams",
        url: "https://zenodo.org/records/7072866",
        download_bytes: Some(667_188_638),
        installed_bytes: None,
        range_resume: false,
        size_checked_at: "2026-07-16T20:00:00-04:00",
        archive_format: "zip-member-streams",
        publisher_md5: None,
        publisher_sha256: None,
    },
];

#[derive(Debug, Clone, Copy)]
pub struct ResourceArtifactTemplate {
    pub id: &'static str,
    pub url_template: &'static str,
    pub filename_template: &'static str,
    pub chromosome_template: bool,
    pub required: bool,
    pub archive_format: &'static str,
    pub download_bytes: Option<u64>,
    pub publisher_md5: Option<&'static str>,
    pub object_sha256: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceCatalogCandidate {
    pub resource_id: &'static str,
    pub version: &'static str,
    pub assembly: &'static str,
    pub provenance: &'static str,
    pub download_bytes: Option<u64>,
    pub artifacts: &'static [ResourceArtifactTemplate],
}

const GNOMAD_V411_EXOME_ARTIFACTS: &[ResourceArtifactTemplate] = &[
    ResourceArtifactTemplate {
        id: "sites",
        url_template: "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/exomes/gnomad.exomes.v4.1.1.sites.chr{chrom}.vcf.bgz",
        filename_template: "gnomad.exomes.v4.1.1.sites.chr{chrom}.vcf.bgz",
        chromosome_template: true,
        required: true,
        archive_format: "bgzip",
        download_bytes: None,
        publisher_md5: None,
        object_sha256: None,
    },
    ResourceArtifactTemplate {
        id: "sites-index",
        url_template: "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/exomes/gnomad.exomes.v4.1.1.sites.chr{chrom}.vcf.bgz.tbi",
        filename_template: "gnomad.exomes.v4.1.1.sites.chr{chrom}.vcf.bgz.tbi",
        chromosome_template: true,
        required: true,
        archive_format: "tabix",
        download_bytes: None,
        publisher_md5: None,
        object_sha256: None,
    },
];

const GNOMAD_V411_GENOME_ARTIFACTS: &[ResourceArtifactTemplate] = &[
    ResourceArtifactTemplate {
        id: "sites",
        url_template: "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/genomes/gnomad.genomes.v4.1.1.sites.chr{chrom}.vcf.bgz",
        filename_template: "gnomad.genomes.v4.1.1.sites.chr{chrom}.vcf.bgz",
        chromosome_template: true,
        required: true,
        archive_format: "bgzip",
        download_bytes: None,
        publisher_md5: None,
        object_sha256: None,
    },
    ResourceArtifactTemplate {
        id: "sites-index",
        url_template: "https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/genomes/gnomad.genomes.v4.1.1.sites.chr{chrom}.vcf.bgz.tbi",
        filename_template: "gnomad.genomes.v4.1.1.sites.chr{chrom}.vcf.bgz.tbi",
        chromosome_template: true,
        required: true,
        archive_format: "tabix",
        download_bytes: None,
        publisher_md5: None,
        object_sha256: None,
    },
];

const REVEL_V13_ARTIFACTS: &[ResourceArtifactTemplate] = &[ResourceArtifactTemplate {
    id: "scores",
    url_template: "https://zenodo.org/records/7072866/files/revel-v1.3_segments_chrom_{chrom_padded}.zip",
    filename_template: "revel-v1.3_segments_chrom_{chrom_padded}.zip",
    chromosome_template: true,
    required: true,
    archive_format: "zip",
    download_bytes: None,
    publisher_md5: None,
    object_sha256: None,
}];

const CADD_V17_ARTIFACTS: &[ResourceArtifactTemplate] = &[
    ResourceArtifactTemplate {
        id: "snv-scores",
        url_template: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/whole_genome_SNVs.tsv.gz",
        filename_template: "whole_genome_SNVs.tsv.gz",
        chromosome_template: false,
        required: true,
        archive_format: "bgzip",
        download_bytes: Some(87_473_403_655),
        publisher_md5: Some("88577a55f1cd519d44e0f415ba248eb9"),
        object_sha256: None,
    },
    ResourceArtifactTemplate {
        id: "snv-index",
        url_template: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/whole_genome_SNVs.tsv.gz.tbi",
        filename_template: "whole_genome_SNVs.tsv.gz.tbi",
        chromosome_template: false,
        required: true,
        archive_format: "tabix",
        download_bytes: Some(2_761_840),
        publisher_md5: Some("347df8fac17ea374c4598f4f44c7ce8b"),
        object_sha256: None,
    },
    ResourceArtifactTemplate {
        id: "indel-scores",
        url_template: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/gnomad.genomes.r4.0.indel.tsv.gz",
        filename_template: "gnomad.genomes.r4.0.indel.tsv.gz",
        chromosome_template: false,
        required: true,
        archive_format: "bgzip",
        download_bytes: Some(1_257_151_321),
        publisher_md5: Some("4b9c685c96d396af4d001c2f7dd9d8f9"),
        object_sha256: None,
    },
    ResourceArtifactTemplate {
        id: "indel-index",
        url_template: "https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/gnomad.genomes.r4.0.indel.tsv.gz.tbi",
        filename_template: "gnomad.genomes.r4.0.indel.tsv.gz.tbi",
        chromosome_template: false,
        required: true,
        archive_format: "tabix",
        download_bytes: Some(1_899_705),
        publisher_md5: Some("85f3d2daa9202c5915c0ce0f1c749a66"),
        object_sha256: None,
    },
];

const PHYLOP100WAY_ARTIFACTS: &[ResourceArtifactTemplate] = &[ResourceArtifactTemplate {
    id: "scores",
    url_template: "https://hgdownload.soe.ucsc.edu/goldenPath/hg38/phyloP100way/hg38.phyloP100way.bw",
    filename_template: "hg38.phyloP100way.bw",
    chromosome_template: false,
    required: true,
    archive_format: "bigwig",
    download_bytes: Some(9_870_053_206),
    publisher_md5: Some("43858006bdf98145b6fd239490bd0478"),
    object_sha256: None,
}];

const ALPHAMISSENSE_2023_ARTIFACTS: &[ResourceArtifactTemplate] = &[ResourceArtifactTemplate {
    id: "scores",
    url_template: "https://zenodo.org/records/8360242/files/AlphaMissense_hg38.tsv.gz?download=1",
    filename_template: "AlphaMissense_hg38.tsv.gz",
    chromosome_template: false,
    required: true,
    archive_format: "gzip",
    download_bytes: Some(642_961_469),
    publisher_md5: Some("9fd167735f16a1b87da6eb3e4c25fcb5"),
    object_sha256: None,
}];

const CLINGEN_20260714_ARTIFACTS: &[ResourceArtifactTemplate] = &[
    ResourceArtifactTemplate {
        id: "gene-dosage",
        url_template: "https://ftp.clinicalgenome.org/archive/20260714/ClinGen_gene_curation_list_GRCh38.tsv",
        filename_template: "ClinGen_gene_curation_list_GRCh38.tsv",
        chromosome_template: false,
        required: true,
        archive_format: "tsv",
        download_bytes: Some(246_399),
        publisher_md5: None,
        object_sha256: Some("f79774379bf6704910196b992a1cc58440eb7a4da1262ca0de17299116c7674b"),
    },
    ResourceArtifactTemplate {
        id: "region-dosage",
        url_template: "https://ftp.clinicalgenome.org/archive/20260714/ClinGen_region_curation_list_GRCh38.tsv",
        filename_template: "ClinGen_region_curation_list_GRCh38.tsv",
        chromosome_template: false,
        required: true,
        archive_format: "tsv",
        download_bytes: Some(98_881),
        publisher_md5: None,
        object_sha256: Some("06bd0333e2c94585c3f823a69be220e07e0c0e1e82dc47e6a325fa734ab8d454"),
    },
];

const GENCC_20260712_ARTIFACTS: &[ResourceArtifactTemplate] = &[ResourceArtifactTemplate {
    id: "submissions",
    url_template: "https://thegencc.org/download/action/submissions-export-tsv?format=new",
    filename_template: "gencc-submissions.tsv",
    chromosome_template: false,
    required: true,
    archive_format: "tsv",
    download_bytes: Some(24_506_017),
    publisher_md5: None,
    object_sha256: Some("5133f0eef3021f2b5b7ea68048e41f765af5824923b4b1ea6efc1acb47a6d50c"),
}];

pub const RESOURCE_CATALOG_CANDIDATES: &[ResourceCatalogCandidate] = &[
    ResourceCatalogCandidate {
        resource_id: "clingen",
        version: "20260714",
        assembly: "GRCh38",
        provenance: "ClinGen dated archive; GRCh38 gene and region dosage curation tables verified as complete TSV objects",
        download_bytes: Some(345_280),
        artifacts: CLINGEN_20260714_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "gencc",
        version: "snapshot-20260712",
        assembly: "gene-level",
        provenance: "GenCC recommended versioned TSV format; CC0 data excluding OMIM; object identity captured from the publisher export dated 2026-07-12",
        download_bytes: Some(24_506_017),
        artifacts: GENCC_20260712_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "alphamissense",
        version: "2023-hg38",
        assembly: "GRCh38",
        provenance: "DeepMind AlphaMissense predictions archived at Zenodo record 8360242; CC BY 4.0 attribution required",
        download_bytes: Some(642_961_469),
        artifacts: ALPHAMISSENSE_2023_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "gnomad",
        version: "4.1.1-exomes",
        assembly: "GRCh38",
        provenance: "Official gnomAD v4.1.1 exome chromosome VCFs verified against publisher sizes and identities",
        download_bytes: Some(199_241_266_182),
        artifacts: GNOMAD_V411_EXOME_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "gnomad-genomes",
        version: "4.1.1-genomes",
        assembly: "GRCh38",
        provenance: "Official gnomAD v4.1.1 genome chromosome VCFs verified against publisher sizes and identities",
        download_bytes: Some(565_643_483_329),
        artifacts: GNOMAD_V411_GENOME_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "revel",
        version: "1.3",
        assembly: "GRCh38",
        provenance: "Official REVEL v1.3 Zenodo record with 24 chromosome archives, publisher MD5 checksums, GRCh38 coordinates, and transcript IDs",
        download_bytes: Some(667_188_638),
        artifacts: REVEL_V13_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "cadd",
        version: "1.7",
        assembly: "GRCh38",
        provenance: "Vera-derived source completed from the official CADD v1.7 download manifest and MD5SUMs",
        download_bytes: Some(88_735_216_521),
        artifacts: CADD_V17_ARTIFACTS,
    },
    ResourceCatalogCandidate {
        resource_id: "phylop",
        version: "hg38-100way-2015-05-08",
        assembly: "GRCh38",
        provenance: "UCSC phyloP100way publisher directory and md5sum.txt",
        download_bytes: Some(9_870_053_206),
        artifacts: PHYLOP100WAY_ARTIFACTS,
    },
];

pub fn resource_catalog_candidates_json() -> String {
    let resources = RESOURCE_CATALOG_CANDIDATES
        .iter()
        .map(|resource| {
            let artifacts = resource
                .artifacts
                .iter()
                .map(|artifact| {
                    format!(
                        "{{\"id\":\"{}\",\"urlTemplate\":\"{}\",\"filenameTemplate\":\"{}\",\"chromosomeTemplate\":{},\"required\":{},\"archiveFormat\":\"{}\",\"downloadBytes\":{},\"publisherMd5\":{},\"objectSha256\":{},\"state\":\"verification-pending\"}}",
                        artifact.id,
                        artifact.url_template,
                        artifact.filename_template,
                        artifact.chromosome_template,
                        artifact.required,
                        artifact.archive_format,
                        artifact
                            .download_bytes
                            .map(|bytes| bytes.to_string())
                            .unwrap_or_else(|| "null".into()),
                        artifact
                            .publisher_md5
                            .map(|md5| format!("\"{md5}\""))
                            .unwrap_or_else(|| "null".into()),
                        artifact
                            .object_sha256
                            .map(|sha256| format!("\"{sha256}\""))
                            .unwrap_or_else(|| "null".into())
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"resourceId\":\"{}\",\"version\":\"{}\",\"assembly\":\"{}\",\"provenance\":\"{}\",\"downloadBytes\":{},\"artifacts\":[{}]}}",
                resource.resource_id,
                resource.version,
                resource.assembly,
                resource.provenance,
                resource
                    .download_bytes
                    .map(|bytes| bytes.to_string())
                    .unwrap_or_else(|| "null".into()),
                artifacts
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{resources}]")
}

pub fn practical_resource_plan_json() -> String {
    let source_ids = [
        "grch38-reference",
        "ensembl-gff3",
        "fastvep",
        "dbnsfp",
        "clinvar",
        "dbsnp",
        "gnomad",
        "gnomad-genomes",
        "cadd",
        "phylop",
        "spliceai",
        "revel",
        "clingen",
        "gencc",
    ];
    let rows = source_ids
        .iter()
        .map(|id| {
            if let Some(release) = RESOURCE_RELEASES.iter().find(|release| release.resource_id == *id) {
                let install_mode = if matches!(*id, "dbnsfp" | "clinvar" | "dbsnp" | "gnomad" | "gnomad-genomes" | "cadd" | "phylop" | "spliceai" | "revel") { "stream" } else { "download" };
                format!("{{\"id\":\"{}\",\"version\":\"{}\",\"filename\":\"{}\",\"downloadBytes\":{},\"installedBytes\":null,\"rangeResume\":{},\"installMode\":\"{}\",\"state\":\"missing\",\"sizeCheckedAt\":\"{}\"}}", release.resource_id, release.version, release.filename, release.download_bytes.unwrap_or(0), release.range_resume, install_mode, release.size_checked_at)
            } else {
                format!("{{\"id\":\"{id}\",\"version\":null,\"filename\":null,\"downloadBytes\":null,\"installedBytes\":null,\"rangeResume\":null,\"state\":\"catalog-pending\"}}")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"profile\":\"practical-wgs\",\"assembly\":\"GRCh38\",\"resources\":[{rows}]}}")
}

#[derive(Debug, Clone)]
pub struct DemoVariant {
    pub chromosome: &'static str,
    pub position: u64,
    pub reference: &'static str,
    pub alternate: &'static str,
    pub gene: &'static str,
    pub consequence: &'static str,
    pub impact: &'static str,
    pub clinvar: &'static str,
    pub inheritance: &'static str,
    pub score: f32,
}

pub const DEMO_VARIANTS: &[DemoVariant] = &[
    DemoVariant {
        chromosome: "1",
        position: 101_001,
        reference: "G",
        alternate: "A",
        gene: "DEMO1",
        consequence: "missense_variant",
        impact: "MODERATE",
        clinvar: "Uncertain significance",
        inheritance: "Autosomal dominant",
        score: 0.82,
    },
    DemoVariant {
        chromosome: "2",
        position: 202_002,
        reference: "C",
        alternate: "T",
        gene: "DEMO2",
        consequence: "stop_gained",
        impact: "HIGH",
        clinvar: "Pathogenic",
        inheritance: "Autosomal recessive",
        score: 0.98,
    },
    DemoVariant {
        chromosome: "X",
        position: 303_003,
        reference: "A",
        alternate: "AT",
        gene: "DEMO3",
        consequence: "frameshift_variant",
        impact: "HIGH",
        clinvar: "Likely pathogenic",
        inheritance: "X-linked",
        score: 0.94,
    },
];

pub fn sources_json() -> String {
    let rows = SOURCES
        .iter()
        .map(|s| {
            let implementation = source_implementation(s.id).expect("source implementation");
            let fastvep_source = implementation
                .fastvep_source
                .map(|source| format!("\"{source}\""))
                .unwrap_or_else(|| "null".into());
            format!(
                "{{\"id\":\"{}\",\"name\":\"{}\",\"purpose\":\"{}\",\"defaultEnabled\":{},\"fastvepSource\":{},\"delivery\":\"{}\",\"assembly\":\"{}\"}}",
                s.id,
                s.name,
                s.purpose,
                s.default_enabled,
                fastvep_source,
                implementation.delivery,
                implementation.assembly
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{}]", rows)
}

pub fn profiles_json() -> String {
    let rows = ANNOTATION_PROFILES
        .iter()
        .map(|profile| {
            let source_ids = profile
                .source_ids
                .iter()
                .map(|source_id| format!("\"{source_id}\""))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"id\":\"{}\",\"name\":\"{}\",\"purpose\":\"{}\",\"sourceIds\":[{}],\"requiredEngineIds\":[\"fastvep\"],\"requiredResourceIds\":[\"grch38-reference\",\"transcript-cache\"]}}",
                profile.id, profile.name, profile.purpose, source_ids
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{rows}]")
}

pub fn demo_variants_json() -> String {
    let rows = DEMO_VARIANTS.iter().map(|v| format!(
        "{{\"chromosome\":\"{}\",\"position\":{},\"reference\":\"{}\",\"alternate\":\"{}\",\"gene\":\"{}\",\"consequence\":\"{}\",\"impact\":\"{}\",\"clinvar\":\"{}\",\"inheritance\":\"{}\",\"score\":{:.2}}}",
        v.chromosome, v.position, v.reference, v.alternate, v.gene, v.consequence, v.impact, v.clinvar, v.inheritance, v.score
    )).collect::<Vec<_>>().join(",");
    format!("[{}]", rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ids_are_unique() {
        for (index, source) in SOURCES.iter().enumerate() {
            assert!(!SOURCES[..index].iter().any(|other| other.id == source.id));
        }
    }

    #[test]
    fn every_source_has_one_explicit_implementation_policy() {
        assert_eq!(SOURCE_IMPLEMENTATIONS.len(), SOURCES.len());
        for source in SOURCES {
            assert_eq!(
                SOURCE_IMPLEMENTATIONS
                    .iter()
                    .filter(|implementation| implementation.source_id == source.id)
                    .count(),
                1,
                "source {} must have exactly one implementation policy",
                source.id
            );
        }
        for restricted in ["omim", "cosmic"] {
            assert_eq!(
                source_implementation(restricted).unwrap().delivery,
                "user-supplied-licensed"
            );
        }
    }

    #[test]
    fn profiles_reference_known_unique_sources() {
        for profile in ANNOTATION_PROFILES {
            for (index, source_id) in profile.source_ids.iter().enumerate() {
                assert!(
                    SOURCES.iter().any(|source| source.id == *source_id),
                    "profile {} references unknown source {}",
                    profile.id,
                    source_id
                );
                assert!(
                    !profile.source_ids[..index].contains(source_id),
                    "profile {} repeats source {}",
                    profile.id,
                    source_id
                );
            }
        }
    }

    #[test]
    fn comprehensive_profile_has_requested_genome_wide_sources() {
        assert_eq!(ANNOTATION_PROFILES[0].id, "wgs");
        let profile = ANNOTATION_PROFILES
            .iter()
            .find(|profile| profile.id == "wgs")
            .unwrap();
        for source_id in ["dbsnp", "cadd", "phylop", "gnomad", "spliceai"] {
            assert!(profile.source_ids.contains(&source_id));
        }
        assert!(!profile.source_ids.contains(&"revel"));
        assert!(!profile.source_ids.contains(&"fastvep"));
    }

    #[test]
    fn pending_predictors_remain_outside_recommended_profiles() {
        for source_id in ["gerp", "primateai", "dann"] {
            let source = SOURCES
                .iter()
                .find(|source| source.id == source_id)
                .unwrap();
            assert!(!source.default_enabled);
            assert!(
                ANNOTATION_PROFILES
                    .iter()
                    .all(|profile| !profile.source_ids.contains(&source_id))
            );
        }
        let minimal = ANNOTATION_PROFILES
            .iter()
            .find(|profile| profile.id == "standard")
            .unwrap();
        assert_eq!(
            minimal.source_ids,
            ["clinvar", "dbsnp", "gnomad", "phylop", "revel"]
        );
        assert!(!minimal.source_ids.contains(&"dbnsfp"));
    }

    #[test]
    fn demo_data_is_explicitly_synthetic() {
        assert!(
            DEMO_VARIANTS
                .iter()
                .all(|variant| variant.gene.starts_with("DEMO"))
        );
    }

    #[test]
    fn dbnsfp_release_has_verified_range_size() {
        let release = RESOURCE_RELEASES
            .iter()
            .find(|release| release.resource_id == "dbnsfp")
            .unwrap();
        assert_eq!(release.download_bytes, Some(38_969_753_349));
        assert!(release.range_resume);
    }

    #[test]
    fn dbsnp_release_is_actionable_through_the_native_parser() {
        let release = RESOURCE_RELEASES
            .iter()
            .find(|release| release.resource_id == "dbsnp")
            .unwrap();
        assert_eq!(release.version, "b157-GRCh38.p14");
        assert_eq!(release.download_bytes, Some(29_552_227_779));
        assert_eq!(
            release.publisher_md5,
            Some("6a6f313e92a39c337571174dad12cfe1")
        );
        assert_eq!(
            source_implementation("dbsnp").unwrap().fastvep_source,
            Some("dbsnp")
        );
    }

    #[test]
    fn alphamissense_candidate_keeps_attribution_and_waits_for_adapter() {
        let candidate = RESOURCE_CATALOG_CANDIDATES
            .iter()
            .find(|candidate| candidate.resource_id == "alphamissense")
            .unwrap();
        assert_eq!(candidate.download_bytes, Some(642_961_469));
        assert_eq!(
            candidate.artifacts[0].publisher_md5,
            Some("9fd167735f16a1b87da6eb3e4c25fcb5")
        );
        assert!(candidate.provenance.contains("CC BY 4.0"));
        assert_eq!(
            source_implementation("alphamissense").unwrap().delivery,
            "adapter-required"
        );
    }

    #[test]
    fn clingen_candidate_pins_both_small_grch38_dosage_tables() {
        let candidate = RESOURCE_CATALOG_CANDIDATES
            .iter()
            .find(|candidate| candidate.resource_id == "clingen")
            .unwrap();
        assert_eq!(candidate.version, "20260714");
        assert_eq!(candidate.download_bytes, Some(345_280));
        assert_eq!(candidate.artifacts.len(), 2);
        assert!(
            candidate
                .artifacts
                .iter()
                .all(|artifact| artifact.object_sha256.is_some())
        );
        assert_eq!(
            source_implementation("clingen").unwrap().delivery,
            "adapter-required"
        );
    }

    #[test]
    fn gencc_candidate_uses_the_new_versioned_cc0_export_schema() {
        let candidate = RESOURCE_CATALOG_CANDIDATES
            .iter()
            .find(|candidate| candidate.resource_id == "gencc")
            .unwrap();
        assert_eq!(candidate.version, "snapshot-20260712");
        assert!(candidate.provenance.contains("CC0"));
        assert!(candidate.provenance.contains("excluding OMIM"));
        assert_eq!(candidate.artifacts[0].download_bytes, Some(24_506_017));
        assert_eq!(
            source_implementation("gencc").unwrap().delivery,
            "adapter-required"
        );
    }

    #[test]
    fn catalog_candidates_remain_descriptive_and_never_drive_downloads_directly() {
        for resource in RESOURCE_CATALOG_CANDIDATES {
            assert!(!resource.artifacts.is_empty());
            for artifact in resource.artifacts {
                assert!(artifact.url_template.starts_with("https://"));
                assert!(!artifact.filename_template.contains(['/', '\\']));
                assert_eq!(
                    artifact.chromosome_template,
                    artifact.url_template.contains("{chrom")
                );
            }
        }
    }
}
