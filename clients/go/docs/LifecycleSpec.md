# LifecycleSpec

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**DeleteOnExit** | Pointer to **bool** | When true, terminated containers (and their bundles) are removed automatically rather than retained for inspection. Defaults to &#x60;false&#x60;, preserving the historical retain-on-exit behavior. | [optional] 

## Methods

### NewLifecycleSpec

`func NewLifecycleSpec() *LifecycleSpec`

NewLifecycleSpec instantiates a new LifecycleSpec object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewLifecycleSpecWithDefaults

`func NewLifecycleSpecWithDefaults() *LifecycleSpec`

NewLifecycleSpecWithDefaults instantiates a new LifecycleSpec object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDeleteOnExit

`func (o *LifecycleSpec) GetDeleteOnExit() bool`

GetDeleteOnExit returns the DeleteOnExit field if non-nil, zero value otherwise.

### GetDeleteOnExitOk

`func (o *LifecycleSpec) GetDeleteOnExitOk() (*bool, bool)`

GetDeleteOnExitOk returns a tuple with the DeleteOnExit field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeleteOnExit

`func (o *LifecycleSpec) SetDeleteOnExit(v bool)`

SetDeleteOnExit sets DeleteOnExit field to given value.

### HasDeleteOnExit

`func (o *LifecycleSpec) HasDeleteOnExit() bool`

HasDeleteOnExit returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


