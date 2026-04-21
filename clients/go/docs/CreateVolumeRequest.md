# CreateVolumeRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Labels** | Pointer to **map[string]string** | Optional labels to attach to the volume. | [optional] 
**Name** | **string** | Volume name. Required. Must match &#x60;^[a-z0-9][a-z0-9_-]{0,63}$&#x60;. | 
**Size** | Pointer to **NullableString** | Optional size hint (humansize format: &#x60;\&quot;512Mi\&quot;&#x60;, &#x60;\&quot;10Gi\&quot;&#x60;). Recorded in the sidecar for display and future quota enforcement. | [optional] 
**Tier** | Pointer to **NullableString** | Optional storage tier. Accepts &#x60;\&quot;local\&quot;&#x60; (default), &#x60;\&quot;cached\&quot;&#x60;, &#x60;\&quot;network\&quot;&#x60;, matching &#x60;zlayer_spec::StorageTier&#x60;. | [optional] 

## Methods

### NewCreateVolumeRequest

`func NewCreateVolumeRequest(name string, ) *CreateVolumeRequest`

NewCreateVolumeRequest instantiates a new CreateVolumeRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateVolumeRequestWithDefaults

`func NewCreateVolumeRequestWithDefaults() *CreateVolumeRequest`

NewCreateVolumeRequestWithDefaults instantiates a new CreateVolumeRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetLabels

`func (o *CreateVolumeRequest) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *CreateVolumeRequest) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *CreateVolumeRequest) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *CreateVolumeRequest) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetName

`func (o *CreateVolumeRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateVolumeRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateVolumeRequest) SetName(v string)`

SetName sets Name field to given value.


### GetSize

`func (o *CreateVolumeRequest) GetSize() string`

GetSize returns the Size field if non-nil, zero value otherwise.

### GetSizeOk

`func (o *CreateVolumeRequest) GetSizeOk() (*string, bool)`

GetSizeOk returns a tuple with the Size field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSize

`func (o *CreateVolumeRequest) SetSize(v string)`

SetSize sets Size field to given value.

### HasSize

`func (o *CreateVolumeRequest) HasSize() bool`

HasSize returns a boolean if a field has been set.

### SetSizeNil

`func (o *CreateVolumeRequest) SetSizeNil(b bool)`

 SetSizeNil sets the value for Size to be an explicit nil

### UnsetSize
`func (o *CreateVolumeRequest) UnsetSize()`

UnsetSize ensures that no value is present for Size, not even an explicit nil
### GetTier

`func (o *CreateVolumeRequest) GetTier() string`

GetTier returns the Tier field if non-nil, zero value otherwise.

### GetTierOk

`func (o *CreateVolumeRequest) GetTierOk() (*string, bool)`

GetTierOk returns a tuple with the Tier field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTier

`func (o *CreateVolumeRequest) SetTier(v string)`

SetTier sets Tier field to given value.

### HasTier

`func (o *CreateVolumeRequest) HasTier() bool`

HasTier returns a boolean if a field has been set.

### SetTierNil

`func (o *CreateVolumeRequest) SetTierNil(b bool)`

 SetTierNil sets the value for Tier to be an explicit nil

### UnsetTier
`func (o *CreateVolumeRequest) UnsetTier()`

UnsetTier ensures that no value is present for Tier, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


