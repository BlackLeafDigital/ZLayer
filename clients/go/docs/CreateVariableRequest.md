# CreateVariableRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Name** | **string** | Variable name (e.g. &#x60;\&quot;APP_VERSION\&quot;&#x60;). Must be unique within the chosen scope. | 
**Scope** | Pointer to **NullableString** | Project id scope. &#x60;None&#x60; &#x3D; global variable. | [optional] 
**Value** | **string** | Plaintext value. | 

## Methods

### NewCreateVariableRequest

`func NewCreateVariableRequest(name string, value string, ) *CreateVariableRequest`

NewCreateVariableRequest instantiates a new CreateVariableRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateVariableRequestWithDefaults

`func NewCreateVariableRequestWithDefaults() *CreateVariableRequest`

NewCreateVariableRequestWithDefaults instantiates a new CreateVariableRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetName

`func (o *CreateVariableRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateVariableRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateVariableRequest) SetName(v string)`

SetName sets Name field to given value.


### GetScope

`func (o *CreateVariableRequest) GetScope() string`

GetScope returns the Scope field if non-nil, zero value otherwise.

### GetScopeOk

`func (o *CreateVariableRequest) GetScopeOk() (*string, bool)`

GetScopeOk returns a tuple with the Scope field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetScope

`func (o *CreateVariableRequest) SetScope(v string)`

SetScope sets Scope field to given value.

### HasScope

`func (o *CreateVariableRequest) HasScope() bool`

HasScope returns a boolean if a field has been set.

### SetScopeNil

`func (o *CreateVariableRequest) SetScopeNil(b bool)`

 SetScopeNil sets the value for Scope to be an explicit nil

### UnsetScope
`func (o *CreateVariableRequest) UnsetScope()`

UnsetScope ensures that no value is present for Scope, not even an explicit nil
### GetValue

`func (o *CreateVariableRequest) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *CreateVariableRequest) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *CreateVariableRequest) SetValue(v string)`

SetValue sets Value field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


