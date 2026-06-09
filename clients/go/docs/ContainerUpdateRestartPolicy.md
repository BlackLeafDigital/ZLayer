# ContainerUpdateRestartPolicy

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**MaximumRetryCount** | Pointer to **NullableInt64** | Maximum number of retries before giving up (only used with &#x60;on-failure&#x60;). When &#x60;0&#x60; or omitted, retries are unbounded. | [optional] 
**Name** | Pointer to **NullableString** | &#x60;\&quot;no\&quot;&#x60;, &#x60;\&quot;always\&quot;&#x60;, &#x60;\&quot;unless-stopped\&quot;&#x60;, or &#x60;\&quot;on-failure\&quot;&#x60;. | [optional] 

## Methods

### NewContainerUpdateRestartPolicy

`func NewContainerUpdateRestartPolicy() *ContainerUpdateRestartPolicy`

NewContainerUpdateRestartPolicy instantiates a new ContainerUpdateRestartPolicy object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerUpdateRestartPolicyWithDefaults

`func NewContainerUpdateRestartPolicyWithDefaults() *ContainerUpdateRestartPolicy`

NewContainerUpdateRestartPolicyWithDefaults instantiates a new ContainerUpdateRestartPolicy object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetMaximumRetryCount

`func (o *ContainerUpdateRestartPolicy) GetMaximumRetryCount() int64`

GetMaximumRetryCount returns the MaximumRetryCount field if non-nil, zero value otherwise.

### GetMaximumRetryCountOk

`func (o *ContainerUpdateRestartPolicy) GetMaximumRetryCountOk() (*int64, bool)`

GetMaximumRetryCountOk returns a tuple with the MaximumRetryCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMaximumRetryCount

`func (o *ContainerUpdateRestartPolicy) SetMaximumRetryCount(v int64)`

SetMaximumRetryCount sets MaximumRetryCount field to given value.

### HasMaximumRetryCount

`func (o *ContainerUpdateRestartPolicy) HasMaximumRetryCount() bool`

HasMaximumRetryCount returns a boolean if a field has been set.

### SetMaximumRetryCountNil

`func (o *ContainerUpdateRestartPolicy) SetMaximumRetryCountNil(b bool)`

 SetMaximumRetryCountNil sets the value for MaximumRetryCount to be an explicit nil

### UnsetMaximumRetryCount
`func (o *ContainerUpdateRestartPolicy) UnsetMaximumRetryCount()`

UnsetMaximumRetryCount ensures that no value is present for MaximumRetryCount, not even an explicit nil
### GetName

`func (o *ContainerUpdateRestartPolicy) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ContainerUpdateRestartPolicy) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ContainerUpdateRestartPolicy) SetName(v string)`

SetName sets Name field to given value.

### HasName

`func (o *ContainerUpdateRestartPolicy) HasName() bool`

HasName returns a boolean if a field has been set.

### SetNameNil

`func (o *ContainerUpdateRestartPolicy) SetNameNil(b bool)`

 SetNameNil sets the value for Name to be an explicit nil

### UnsetName
`func (o *ContainerUpdateRestartPolicy) UnsetName()`

UnsetName ensures that no value is present for Name, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


