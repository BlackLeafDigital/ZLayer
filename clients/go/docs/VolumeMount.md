# VolumeMount

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Readonly** | Pointer to **bool** | Mount as read-only | [optional] 
**Source** | Pointer to **NullableString** | Host path (bind), volume name (volume), or unused (tmpfs). | [optional] 
**Target** | **string** | Container mount path | 
**Type** | Pointer to [**NullableVolumeMountType**](VolumeMountType.md) | Mount kind. Omit (or &#x60;\&quot;bind\&quot;&#x60;) for legacy host-path binds. | [optional] 

## Methods

### NewVolumeMount

`func NewVolumeMount(target string, ) *VolumeMount`

NewVolumeMount instantiates a new VolumeMount object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewVolumeMountWithDefaults

`func NewVolumeMountWithDefaults() *VolumeMount`

NewVolumeMountWithDefaults instantiates a new VolumeMount object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetReadonly

`func (o *VolumeMount) GetReadonly() bool`

GetReadonly returns the Readonly field if non-nil, zero value otherwise.

### GetReadonlyOk

`func (o *VolumeMount) GetReadonlyOk() (*bool, bool)`

GetReadonlyOk returns a tuple with the Readonly field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReadonly

`func (o *VolumeMount) SetReadonly(v bool)`

SetReadonly sets Readonly field to given value.

### HasReadonly

`func (o *VolumeMount) HasReadonly() bool`

HasReadonly returns a boolean if a field has been set.

### GetSource

`func (o *VolumeMount) GetSource() string`

GetSource returns the Source field if non-nil, zero value otherwise.

### GetSourceOk

`func (o *VolumeMount) GetSourceOk() (*string, bool)`

GetSourceOk returns a tuple with the Source field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSource

`func (o *VolumeMount) SetSource(v string)`

SetSource sets Source field to given value.

### HasSource

`func (o *VolumeMount) HasSource() bool`

HasSource returns a boolean if a field has been set.

### SetSourceNil

`func (o *VolumeMount) SetSourceNil(b bool)`

 SetSourceNil sets the value for Source to be an explicit nil

### UnsetSource
`func (o *VolumeMount) UnsetSource()`

UnsetSource ensures that no value is present for Source, not even an explicit nil
### GetTarget

`func (o *VolumeMount) GetTarget() string`

GetTarget returns the Target field if non-nil, zero value otherwise.

### GetTargetOk

`func (o *VolumeMount) GetTargetOk() (*string, bool)`

GetTargetOk returns a tuple with the Target field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTarget

`func (o *VolumeMount) SetTarget(v string)`

SetTarget sets Target field to given value.


### GetType

`func (o *VolumeMount) GetType() VolumeMountType`

GetType returns the Type field if non-nil, zero value otherwise.

### GetTypeOk

`func (o *VolumeMount) GetTypeOk() (*VolumeMountType, bool)`

GetTypeOk returns a tuple with the Type field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetType

`func (o *VolumeMount) SetType(v VolumeMountType)`

SetType sets Type field to given value.

### HasType

`func (o *VolumeMount) HasType() bool`

HasType returns a boolean if a field has been set.

### SetTypeNil

`func (o *VolumeMount) SetTypeNil(b bool)`

 SetTypeNil sets the value for Type to be an explicit nil

### UnsetType
`func (o *VolumeMount) UnsetType()`

UnsetType ensures that no value is present for Type, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


