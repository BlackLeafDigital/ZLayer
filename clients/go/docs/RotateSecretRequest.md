# RotateSecretRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**NodeAffinity** | Pointer to [**NullableNodeAffinity**](NodeAffinity.md) | Optional per-secret node affinity update. &#x60;None&#x60; here means \&quot;leave existing affinity unchanged\&quot;; to clear affinity explicitly, pass an empty selector via a separate update endpoint (Phase 2). | [optional] 
**Value** | **string** | The new secret value (will be encrypted at rest). | 

## Methods

### NewRotateSecretRequest

`func NewRotateSecretRequest(value string, ) *RotateSecretRequest`

NewRotateSecretRequest instantiates a new RotateSecretRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRotateSecretRequestWithDefaults

`func NewRotateSecretRequestWithDefaults() *RotateSecretRequest`

NewRotateSecretRequestWithDefaults instantiates a new RotateSecretRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetNodeAffinity

`func (o *RotateSecretRequest) GetNodeAffinity() NodeAffinity`

GetNodeAffinity returns the NodeAffinity field if non-nil, zero value otherwise.

### GetNodeAffinityOk

`func (o *RotateSecretRequest) GetNodeAffinityOk() (*NodeAffinity, bool)`

GetNodeAffinityOk returns a tuple with the NodeAffinity field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeAffinity

`func (o *RotateSecretRequest) SetNodeAffinity(v NodeAffinity)`

SetNodeAffinity sets NodeAffinity field to given value.

### HasNodeAffinity

`func (o *RotateSecretRequest) HasNodeAffinity() bool`

HasNodeAffinity returns a boolean if a field has been set.

### SetNodeAffinityNil

`func (o *RotateSecretRequest) SetNodeAffinityNil(b bool)`

 SetNodeAffinityNil sets the value for NodeAffinity to be an explicit nil

### UnsetNodeAffinity
`func (o *RotateSecretRequest) UnsetNodeAffinity()`

UnsetNodeAffinity ensures that no value is present for NodeAffinity, not even an explicit nil
### GetValue

`func (o *RotateSecretRequest) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *RotateSecretRequest) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *RotateSecretRequest) SetValue(v string)`

SetValue sets Value field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


