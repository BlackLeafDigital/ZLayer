# ClusterUpgradeRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CooldownSecs** | Pointer to **int64** | Seconds to pause between node upgrades after each follower comes back healthy. | [optional] 
**Strict** | Pointer to **bool** | Abort the rollout if any follower fails to come back healthy. | [optional] 
**Version** | Pointer to **NullableString** | Target zlayer version (e.g. \&quot;v0.12.0\&quot;). Defaults to latest GitHub release if absent. | [optional] 

## Methods

### NewClusterUpgradeRequest

`func NewClusterUpgradeRequest() *ClusterUpgradeRequest`

NewClusterUpgradeRequest instantiates a new ClusterUpgradeRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterUpgradeRequestWithDefaults

`func NewClusterUpgradeRequestWithDefaults() *ClusterUpgradeRequest`

NewClusterUpgradeRequestWithDefaults instantiates a new ClusterUpgradeRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCooldownSecs

`func (o *ClusterUpgradeRequest) GetCooldownSecs() int64`

GetCooldownSecs returns the CooldownSecs field if non-nil, zero value otherwise.

### GetCooldownSecsOk

`func (o *ClusterUpgradeRequest) GetCooldownSecsOk() (*int64, bool)`

GetCooldownSecsOk returns a tuple with the CooldownSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCooldownSecs

`func (o *ClusterUpgradeRequest) SetCooldownSecs(v int64)`

SetCooldownSecs sets CooldownSecs field to given value.

### HasCooldownSecs

`func (o *ClusterUpgradeRequest) HasCooldownSecs() bool`

HasCooldownSecs returns a boolean if a field has been set.

### GetStrict

`func (o *ClusterUpgradeRequest) GetStrict() bool`

GetStrict returns the Strict field if non-nil, zero value otherwise.

### GetStrictOk

`func (o *ClusterUpgradeRequest) GetStrictOk() (*bool, bool)`

GetStrictOk returns a tuple with the Strict field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStrict

`func (o *ClusterUpgradeRequest) SetStrict(v bool)`

SetStrict sets Strict field to given value.

### HasStrict

`func (o *ClusterUpgradeRequest) HasStrict() bool`

HasStrict returns a boolean if a field has been set.

### GetVersion

`func (o *ClusterUpgradeRequest) GetVersion() string`

GetVersion returns the Version field if non-nil, zero value otherwise.

### GetVersionOk

`func (o *ClusterUpgradeRequest) GetVersionOk() (*string, bool)`

GetVersionOk returns a tuple with the Version field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVersion

`func (o *ClusterUpgradeRequest) SetVersion(v string)`

SetVersion sets Version field to given value.

### HasVersion

`func (o *ClusterUpgradeRequest) HasVersion() bool`

HasVersion returns a boolean if a field has been set.

### SetVersionNil

`func (o *ClusterUpgradeRequest) SetVersionNil(b bool)`

 SetVersionNil sets the value for Version to be an explicit nil

### UnsetVersion
`func (o *ClusterUpgradeRequest) UnsetVersion()`

UnsetVersion ensures that no value is present for Version, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


