# UpgradeStartRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Version** | Pointer to **NullableString** | Target version. &#x60;None&#x60; (or &#x60;\&quot;latest\&quot;&#x60;) defers to &#x60;self-update&#x60;&#39;s \&quot;latest GitHub release\&quot; resolver. | [optional] 

## Methods

### NewUpgradeStartRequest

`func NewUpgradeStartRequest() *UpgradeStartRequest`

NewUpgradeStartRequest instantiates a new UpgradeStartRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewUpgradeStartRequestWithDefaults

`func NewUpgradeStartRequestWithDefaults() *UpgradeStartRequest`

NewUpgradeStartRequestWithDefaults instantiates a new UpgradeStartRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetVersion

`func (o *UpgradeStartRequest) GetVersion() string`

GetVersion returns the Version field if non-nil, zero value otherwise.

### GetVersionOk

`func (o *UpgradeStartRequest) GetVersionOk() (*string, bool)`

GetVersionOk returns a tuple with the Version field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVersion

`func (o *UpgradeStartRequest) SetVersion(v string)`

SetVersion sets Version field to given value.

### HasVersion

`func (o *UpgradeStartRequest) HasVersion() bool`

HasVersion returns a boolean if a field has been set.

### SetVersionNil

`func (o *UpgradeStartRequest) SetVersionNil(b bool)`

 SetVersionNil sets the value for Version to be an explicit nil

### UnsetVersion
`func (o *UpgradeStartRequest) UnsetVersion()`

UnsetVersion ensures that no value is present for Version, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


