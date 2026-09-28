-- The storage account an Azure Blob profile's containers live in (RD-150-05). An identifier,
-- not a secret; NULL for S3 and Google Cloud Storage, whose links name the bucket alone.
ALTER TABLE object_storage_profiles ADD COLUMN account TEXT;
