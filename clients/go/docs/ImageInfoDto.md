# ImageInfoDto

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Digest** | Pointer to **NullableString** | Content-addressed digest (&#x60;sha256:...&#x60;) if known. | [optional] 
**Reference** | **string** | Canonical image reference (e.g. &#x60;zachhandley/zlayer-manager:latest&#x60;). | 
**SizeBytes** | Pointer to **NullableInt64** | Size in bytes if known. | [optional] 

## Methods

### NewImageInfoDto

`func NewImageInfoDto(reference string, ) *ImageInfoDto`

NewImageInfoDto instantiates a new ImageInfoDto object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewImageInfoDtoWithDefaults

`func NewImageInfoDtoWithDefaults() *ImageInfoDto`

NewImageInfoDtoWithDefaults instantiates a new ImageInfoDto object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDigest

`func (o *ImageInfoDto) GetDigest() string`

GetDigest returns the Digest field if non-nil, zero value otherwise.

### GetDigestOk

`func (o *ImageInfoDto) GetDigestOk() (*string, bool)`

GetDigestOk returns a tuple with the Digest field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDigest

`func (o *ImageInfoDto) SetDigest(v string)`

SetDigest sets Digest field to given value.

### HasDigest

`func (o *ImageInfoDto) HasDigest() bool`

HasDigest returns a boolean if a field has been set.

### SetDigestNil

`func (o *ImageInfoDto) SetDigestNil(b bool)`

 SetDigestNil sets the value for Digest to be an explicit nil

### UnsetDigest
`func (o *ImageInfoDto) UnsetDigest()`

UnsetDigest ensures that no value is present for Digest, not even an explicit nil
### GetReference

`func (o *ImageInfoDto) GetReference() string`

GetReference returns the Reference field if non-nil, zero value otherwise.

### GetReferenceOk

`func (o *ImageInfoDto) GetReferenceOk() (*string, bool)`

GetReferenceOk returns a tuple with the Reference field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReference

`func (o *ImageInfoDto) SetReference(v string)`

SetReference sets Reference field to given value.


### GetSizeBytes

`func (o *ImageInfoDto) GetSizeBytes() int64`

GetSizeBytes returns the SizeBytes field if non-nil, zero value otherwise.

### GetSizeBytesOk

`func (o *ImageInfoDto) GetSizeBytesOk() (*int64, bool)`

GetSizeBytesOk returns a tuple with the SizeBytes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSizeBytes

`func (o *ImageInfoDto) SetSizeBytes(v int64)`

SetSizeBytes sets SizeBytes field to given value.

### HasSizeBytes

`func (o *ImageInfoDto) HasSizeBytes() bool`

HasSizeBytes returns a boolean if a field has been set.

### SetSizeBytesNil

`func (o *ImageInfoDto) SetSizeBytesNil(b bool)`

 SetSizeBytesNil sets the value for SizeBytes to be an explicit nil

### UnsetSizeBytes
`func (o *ImageInfoDto) UnsetSizeBytes()`

UnsetSizeBytes ensures that no value is present for SizeBytes, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


