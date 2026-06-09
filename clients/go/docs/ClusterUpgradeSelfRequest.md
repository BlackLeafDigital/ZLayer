# ClusterUpgradeSelfRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Version** | Pointer to **NullableString** | Target version (e.g. &#x60;\&quot;v0.12.0\&quot;&#x60;). Defaults to latest GitHub release. | [optional] 

## Methods

### NewClusterUpgradeSelfRequest

`func NewClusterUpgradeSelfRequest() *ClusterUpgradeSelfRequest`

NewClusterUpgradeSelfRequest instantiates a new ClusterUpgradeSelfRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterUpgradeSelfRequestWithDefaults

`func NewClusterUpgradeSelfRequestWithDefaults() *ClusterUpgradeSelfRequest`

NewClusterUpgradeSelfRequestWithDefaults instantiates a new ClusterUpgradeSelfRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetVersion

`func (o *ClusterUpgradeSelfRequest) GetVersion() string`

GetVersion returns the Version field if non-nil, zero value otherwise.

### GetVersionOk

`func (o *ClusterUpgradeSelfRequest) GetVersionOk() (*string, bool)`

GetVersionOk returns a tuple with the Version field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVersion

`func (o *ClusterUpgradeSelfRequest) SetVersion(v string)`

SetVersion sets Version field to given value.

### HasVersion

`func (o *ClusterUpgradeSelfRequest) HasVersion() bool`

HasVersion returns a boolean if a field has been set.

### SetVersionNil

`func (o *ClusterUpgradeSelfRequest) SetVersionNil(b bool)`

 SetVersionNil sets the value for Version to be an explicit nil

### UnsetVersion
`func (o *ClusterUpgradeSelfRequest) UnsetVersion()`

UnsetVersion ensures that no value is present for Version, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


