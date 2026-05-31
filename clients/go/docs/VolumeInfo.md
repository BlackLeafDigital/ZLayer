# VolumeInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | RFC 3339 creation timestamp. For volumes without a sidecar this is the directory&#39;s mtime (best-effort). | 
**InUseBy** | Pointer to **[]string** | Container IDs currently mounting this volume. Empty when no &#x60;VolumeUsageSource&#x60; is wired. | [optional] 
**Labels** | Pointer to **map[string]string** | Labels from the sidecar. Empty when no sidecar is present. | [optional] 
**Name** | **string** | Volume name (directory name). | 
**Path** | **string** | Host filesystem path. | 
**SizeBytes** | Pointer to **NullableInt64** | Approximate size in bytes (sum of regular files in the volume directory). &#x60;None&#x60; when the directory could not be walked, or for freshly created empty volumes where &#x60;0&#x60; would be equally informative. | [optional] 

## Methods

### NewVolumeInfo

`func NewVolumeInfo(createdAt string, name string, path string, ) *VolumeInfo`

NewVolumeInfo instantiates a new VolumeInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewVolumeInfoWithDefaults

`func NewVolumeInfoWithDefaults() *VolumeInfo`

NewVolumeInfoWithDefaults instantiates a new VolumeInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *VolumeInfo) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *VolumeInfo) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *VolumeInfo) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetInUseBy

`func (o *VolumeInfo) GetInUseBy() []string`

GetInUseBy returns the InUseBy field if non-nil, zero value otherwise.

### GetInUseByOk

`func (o *VolumeInfo) GetInUseByOk() (*[]string, bool)`

GetInUseByOk returns a tuple with the InUseBy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetInUseBy

`func (o *VolumeInfo) SetInUseBy(v []string)`

SetInUseBy sets InUseBy field to given value.

### HasInUseBy

`func (o *VolumeInfo) HasInUseBy() bool`

HasInUseBy returns a boolean if a field has been set.

### GetLabels

`func (o *VolumeInfo) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *VolumeInfo) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *VolumeInfo) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *VolumeInfo) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetName

`func (o *VolumeInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *VolumeInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *VolumeInfo) SetName(v string)`

SetName sets Name field to given value.


### GetPath

`func (o *VolumeInfo) GetPath() string`

GetPath returns the Path field if non-nil, zero value otherwise.

### GetPathOk

`func (o *VolumeInfo) GetPathOk() (*string, bool)`

GetPathOk returns a tuple with the Path field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPath

`func (o *VolumeInfo) SetPath(v string)`

SetPath sets Path field to given value.


### GetSizeBytes

`func (o *VolumeInfo) GetSizeBytes() int64`

GetSizeBytes returns the SizeBytes field if non-nil, zero value otherwise.

### GetSizeBytesOk

`func (o *VolumeInfo) GetSizeBytesOk() (*int64, bool)`

GetSizeBytesOk returns a tuple with the SizeBytes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSizeBytes

`func (o *VolumeInfo) SetSizeBytes(v int64)`

SetSizeBytes sets SizeBytes field to given value.

### HasSizeBytes

`func (o *VolumeInfo) HasSizeBytes() bool`

HasSizeBytes returns a boolean if a field has been set.

### SetSizeBytesNil

`func (o *VolumeInfo) SetSizeBytesNil(b bool)`

 SetSizeBytesNil sets the value for SizeBytes to be an explicit nil

### UnsetSizeBytes
`func (o *VolumeInfo) UnsetSizeBytes()`

UnsetSizeBytes ensures that no value is present for SizeBytes, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


